//! Worktree registry enumeration and parallel status computation.
//!
//! All reads go through `git2`; this module NEVER spawns a `git` process.
//! Cheap fields (name/path/branch/HEAD) are resolved from the registry and
//! the shared ref store with a single open of the main repository. Expensive
//! status is computed in parallel with rayon, opening a fresh
//! `git2::Repository` per worktree inside each task (`Repository` is not
//! `Sync`).
//!
//! Status semantics:
//! - **dirty**: any entry in `repo.statuses` with `include_untracked(true)`,
//!   ignored files excluded, `exclude_submodules(true)`.
//! - **ahead/behind**: `branch.upstream()` + `graph_ahead_behind`; both are
//!   `None` when the branch has no upstream.
//! - **upstream_gone**: the branch has upstream configuration
//!   (`branch.<name>.merge` in config) but the upstream ref no longer
//!   exists.
//! - **merged**: the branch tip is an ancestor of (or equal to) the resolved
//!   base tip. The base is resolved once in the MAIN repository from
//!   [`ListOptions::base`] via revparse, falling back to the main worktree's
//!   HEAD when unset or unresolvable. A worktree whose branch IS the base
//!   (shorthand match) is never flagged merged.

use std::fs;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::error::{Error, Result};
use crate::model::{WorktreeInfo, WorktreeStatus};
use crate::repo::RepoContext;

/// Options for listing.
#[derive(Debug, Clone, Default)]
pub struct ListOptions {
    /// Compute per-worktree status (dirty/ahead/behind/gone/merged) in
    /// parallel with rayon. When false, `WorktreeInfo.status` is None.
    pub with_status: bool,
    /// Base ref for merged detection, e.g. "origin/main" (config
    /// default_base). Resolved via revparse in the main repo; falls back to
    /// the main worktree HEAD when None/unresolvable.
    pub base: Option<String>,
}

/// Enumerate ALL worktrees from git's own registry: the main worktree first,
/// then every linked worktree (wherever it lives on disk), including entries
/// created by raw `git worktree add`. Registry entries whose directory has
/// been moved/deleted are returned with `is_missing = true` (never an error).
/// Status computation uses rayon par_iter, opening a git2 Repository per
/// worktree; missing worktrees get `status: None`.
pub fn list(ctx: &RepoContext, opts: &ListOptions) -> Result<Vec<WorktreeInfo>> {
    let main_repo = ctx.open_main()?;

    let mut infos = vec![main_info(ctx, &main_repo)];
    let names = main_repo.worktrees()?;
    for name in names.iter().filter_map(|n| n.ok().flatten()) {
        infos.push(linked_info(ctx, &main_repo, name));
    }

    if opts.with_status {
        let base = resolve_base(&main_repo, opts.base.as_deref());
        drop(main_repo);
        infos.par_iter_mut().for_each(|info| {
            if !info.is_missing {
                info.status = compute_status(&info.path, info.branch.as_deref(), base.as_ref());
            }
        });
    }

    Ok(infos)
}

/// Resolve `<name>` to a worktree: exact match on registry name, then branch
/// name, then unique substring of branch/name (error WorktreeNotFound
/// otherwise; if substring matching is ambiguous, also WorktreeNotFound with
/// the candidates listed in the message). Never computes status.
pub fn find(ctx: &RepoContext, name: &str) -> Result<WorktreeInfo> {
    let infos = list(ctx, &ListOptions::default())?;

    if let Some(info) = infos.iter().find(|i| i.name == name) {
        return Ok(info.clone());
    }
    if let Some(info) = infos.iter().find(|i| i.branch.as_deref() == Some(name)) {
        return Ok(info.clone());
    }

    let matches: Vec<&WorktreeInfo> = infos
        .iter()
        .filter(|i| i.name.contains(name) || i.display_name().contains(name))
        .collect();
    match matches.as_slice() {
        [single] => Ok((*single).clone()),
        [] => Err(Error::WorktreeNotFound(name.to_string())),
        many => {
            let candidates = many
                .iter()
                .map(|i| i.display_name())
                .collect::<Vec<_>>()
                .join(", ");
            Err(Error::WorktreeNotFound(format!(
                "{name} (ambiguous: matches {candidates})"
            )))
        }
    }
}

/// Worktree containing `path` (used to detect "you are removing the worktree
/// you are standing in"). None if path is in no known worktree.
pub fn containing(ctx: &RepoContext, path: &Path) -> Result<Option<WorktreeInfo>> {
    let infos = list(ctx, &ListOptions::default())?;
    let probe = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    Ok(infos
        .into_iter()
        .filter(|i| !i.is_missing && probe.starts_with(&i.path))
        .max_by_key(|i| i.path.components().count()))
}

/// Base tip resolved once in the main repository; shared read-only across the
/// rayon status tasks (only an `Oid` and owned strings, both `Sync`).
struct ResolvedBase {
    oid: git2::Oid,
    /// Names identifying the base itself (the raw spec and the resolved
    /// reference shorthand): a worktree whose branch matches one of these is
    /// never flagged merged.
    names: Vec<String>,
}

/// Resolve the merged-detection base in the MAIN repo: revparse `base` when
/// given; otherwise (or when unresolvable) the main worktree's HEAD. Returns
/// None only when even HEAD cannot be resolved (e.g. unborn branch).
fn resolve_base(repo: &git2::Repository, base: Option<&str>) -> Option<ResolvedBase> {
    if let Some(spec) = base {
        if let Ok((object, reference)) = repo.revparse_ext(spec) {
            if let Ok(commit) = object.peel(git2::ObjectType::Commit) {
                let mut names = vec![spec.to_string()];
                if let Some(short) = reference.as_ref().and_then(|r| r.shorthand().ok()) {
                    if short != spec {
                        names.push(short.to_string());
                    }
                }
                return Some(ResolvedBase {
                    oid: commit.id(),
                    names,
                });
            }
        }
    }

    let head = repo.head().ok()?;
    let oid = head.peel_to_commit().ok()?.id();
    let mut names = Vec::new();
    if head.is_branch() {
        if let Ok(short) = head.shorthand() {
            names.push(short.to_string());
        }
    }
    Some(ResolvedBase { oid, names })
}

/// Compute expensive status for one worktree by opening a fresh repository at
/// `path` (safe to call from a rayon task). Returns None when the worktree
/// cannot be opened; individual sub-checks degrade to their zero values
/// rather than failing the whole listing.
fn compute_status(
    path: &Path,
    branch: Option<&str>,
    base: Option<&ResolvedBase>,
) -> Option<WorktreeStatus> {
    let repo = git2::Repository::open(path).ok()?;

    // Dirty: any status entry, untracked included, ignored and submodules
    // excluded.
    let mut status_opts = git2::StatusOptions::new();
    status_opts
        .include_untracked(true)
        .include_ignored(false)
        .exclude_submodules(true);
    let dirty = repo
        .statuses(Some(&mut status_opts))
        .map(|s| s.iter().next().is_some())
        .unwrap_or(false);

    let mut ahead = None;
    let mut behind = None;
    let mut upstream_gone = false;
    let mut tip: Option<git2::Oid> = None;

    if let Some(name) = branch {
        if let Ok(local) = repo.find_branch(name, git2::BranchType::Local) {
            tip = local.get().target();
            match local.upstream() {
                Ok(upstream) => {
                    if let (Some(local_oid), Some(upstream_oid)) = (tip, upstream.get().target()) {
                        if let Ok((a, b)) = repo.graph_ahead_behind(local_oid, upstream_oid) {
                            ahead = Some(a);
                            behind = Some(b);
                        }
                    }
                }
                Err(e) if e.code() == git2::ErrorCode::NotFound => {
                    // Upstream config present but the ref itself is gone.
                    upstream_gone = repo
                        .config()
                        .and_then(|cfg| cfg.get_string(&format!("branch.{name}.merge")))
                        .is_ok();
                }
                Err(_) => {}
            }
        }
    }

    // Detached HEAD (or unreadable branch ref): fall back to HEAD's commit.
    if tip.is_none() {
        tip = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .map(|c| c.id());
    }

    let merged = match (tip, base) {
        (Some(tip_oid), Some(base_ref)) => {
            let is_base_itself =
                branch.is_some_and(|name| base_ref.names.iter().any(|n| n == name));
            if is_base_itself {
                false
            } else {
                tip_oid == base_ref.oid
                    || repo
                        .graph_descendant_of(base_ref.oid, tip_oid)
                        .unwrap_or(false)
            }
        }
        _ => false,
    };

    Some(WorktreeStatus {
        dirty,
        ahead,
        behind,
        upstream_gone,
        merged,
    })
}

/// Info for the main working tree (always listed first, name "main").
fn main_info(ctx: &RepoContext, repo: &git2::Repository) -> WorktreeInfo {
    let head = repo.head().ok();
    let branch = head
        .as_ref()
        .filter(|h| h.is_branch())
        .and_then(|h| h.shorthand().ok())
        .map(str::to_owned);
    let head_short = head
        .as_ref()
        .and_then(|h| h.peel_to_commit().ok())
        .and_then(|c| short_id(repo, c.id()));

    WorktreeInfo {
        name: "main".to_string(),
        path: ctx.main_root.clone(),
        branch,
        head: head_short,
        is_main: true,
        is_missing: !ctx.main_root.exists(),
        is_locked: false,
        is_prunable: false,
        status: None,
    }
}

/// Info for one linked registry entry. Never fails: broken or moved/deleted
/// entries degrade to `is_missing: true` with whatever metadata can still be
/// recovered textually from the registry.
fn linked_info(ctx: &RepoContext, main_repo: &git2::Repository, name: &str) -> WorktreeInfo {
    let (path, is_locked, is_prunable) = match main_repo.find_worktree(name) {
        Ok(wt) => {
            let locked = matches!(wt.is_locked(), Ok(git2::WorktreeLockStatus::Locked(_)));
            let prunable = wt.is_prunable(None).unwrap_or(false);
            (Some(wt.path().to_path_buf()), locked, prunable)
        }
        Err(_) => (registered_path(&ctx.git_dir, name), false, true),
    };

    let (path, is_missing) = match path {
        Some(p) if p.exists() => (fs::canonicalize(&p).unwrap_or(p), false),
        Some(p) => (p, true),
        None => (ctx.git_dir.join("worktrees").join(name), true),
    };

    let (branch, head) = head_info(ctx, main_repo, name);

    WorktreeInfo {
        name: name.to_string(),
        path,
        branch,
        head,
        is_main: false,
        is_missing,
        is_locked,
        is_prunable,
        status: None,
    }
}

/// Recover a registry entry's worktree path textually from
/// `git_dir/worktrees/<name>/gitdir` (which stores `<worktree>/.git`).
fn registered_path(git_dir: &Path, name: &str) -> Option<PathBuf> {
    let content = fs::read_to_string(git_dir.join("worktrees").join(name).join("gitdir")).ok()?;
    let dotgit = PathBuf::from(content.trim());
    dotgit.parent().map(Path::to_path_buf)
}

/// Cheap branch + short HEAD sha for a linked worktree, read textually from
/// `git_dir/worktrees/<name>/HEAD` (works even when the worktree directory
/// itself was deleted) and resolved through the main repo's shared ref/odb
/// store — no per-worktree repository open.
fn head_info(
    ctx: &RepoContext,
    main_repo: &git2::Repository,
    name: &str,
) -> (Option<String>, Option<String>) {
    let head_file = ctx.git_dir.join("worktrees").join(name).join("HEAD");
    let Ok(content) = fs::read_to_string(&head_file) else {
        return (None, None);
    };
    let line = content.lines().next().unwrap_or("").trim();

    if let Some(refname) = line.strip_prefix("ref: ") {
        let branch = refname.strip_prefix("refs/heads/").map(str::to_owned);
        let oid = main_repo
            .find_reference(refname)
            .ok()
            .and_then(|r| r.resolve().ok())
            .and_then(|r| r.target());
        (branch, oid.and_then(|o| short_id(main_repo, o)))
    } else {
        // Detached HEAD: the file holds the raw commit id.
        let oid = git2::Oid::from_str(line).ok();
        (None, oid.and_then(|o| short_id(main_repo, o)))
    }
}

/// Abbreviated (7+ chars, uniqueness-extended) object id via `short_id`.
fn short_id(repo: &git2::Repository, oid: git2::Oid) -> Option<String> {
    let object = repo.find_object(oid, None).ok()?;
    let buf = object.short_id().ok()?;
    buf.as_str().ok().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "wtm test")
            .env("GIT_AUTHOR_EMAIL", "wtm@example.invalid")
            .env("GIT_COMMITTER_NAME", "wtm test")
            .env("GIT_COMMITTER_EMAIL", "wtm@example.invalid")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("failed to run git");
        assert!(
            out.status.success(),
            "git {:?} failed:\n{}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn commit_file(dir: &Path, file: &str) {
        fs::write(dir.join(file), file).unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-m", file]);
    }

    /// tmp dir with `main/` containing an initialized repo (branch "main",
    /// one commit).
    fn fixture() -> (tempfile::TempDir, RepoContext) {
        let tmp = tempfile::TempDir::new().unwrap();
        let main = tmp.path().join("main");
        fs::create_dir(&main).unwrap();
        git(&main, &["init", "-b", "main"]);
        commit_file(&main, "README.md");
        let ctx = crate::repo::discover(Some(&main)).unwrap();
        (tmp, ctx)
    }

    fn add_worktree(ctx: &RepoContext, dest: &Path, branch: &str) {
        git(
            &ctx.main_root,
            &["worktree", "add", "-b", branch, dest.to_str().unwrap()],
        );
    }

    fn status_opts() -> ListOptions {
        ListOptions {
            with_status: true,
            base: None,
        }
    }

    fn entry<'a>(infos: &'a [WorktreeInfo], name: &str) -> &'a WorktreeInfo {
        infos
            .iter()
            .find(|i| i.name == name)
            .unwrap_or_else(|| panic!("no entry named {name}"))
    }

    #[test]
    fn lists_main_first_without_status() {
        let (_tmp, ctx) = fixture();
        let infos = list(&ctx, &ListOptions::default()).unwrap();
        assert_eq!(infos.len(), 1);
        let main = &infos[0];
        assert!(main.is_main);
        assert_eq!(main.name, "main");
        assert_eq!(main.branch.as_deref(), Some("main"));
        assert!(main.head.as_ref().is_some_and(|h| h.len() >= 7));
        assert!(main.status.is_none());
    }

    #[test]
    fn lists_worktree_added_at_arbitrary_path() {
        let (tmp, ctx) = fixture();
        // Deliberately NOT a template-shaped location.
        let dest = tmp.path().join("elsewhere").join("deep").join("feat");
        add_worktree(&ctx, &dest, "feat");
        // Plus a detached worktree.
        let det = tmp.path().join("det");
        git(
            &ctx.main_root,
            &["worktree", "add", "--detach", det.to_str().unwrap()],
        );

        let infos = list(&ctx, &ListOptions::default()).unwrap();
        assert_eq!(infos.len(), 3);
        assert!(infos[0].is_main);

        let feat = entry(&infos, "feat");
        assert_eq!(feat.branch.as_deref(), Some("feat"));
        assert_eq!(feat.path, fs::canonicalize(&dest).unwrap());
        assert!(!feat.is_missing);
        assert!(!feat.is_main);
        assert!(feat.head.is_some());

        let det = entry(&infos, "det");
        assert_eq!(det.branch, None, "detached HEAD has no branch");
        assert!(det.head.is_some());
    }

    #[test]
    fn missing_worktree_is_listed_not_an_error() {
        let (tmp, ctx) = fixture();
        let dest = tmp.path().join("wts").join("gone");
        add_worktree(&ctx, &dest, "doomed");
        fs::remove_dir_all(&dest).unwrap();

        let infos = list(&ctx, &status_opts()).unwrap();
        let gone = entry(&infos, "gone");
        assert!(gone.is_missing);
        // Branch recovered textually from git_dir/worktrees/gone/HEAD.
        assert_eq!(gone.branch.as_deref(), Some("doomed"));
        assert!(gone.is_prunable);
        assert!(gone.status.is_none(), "missing worktrees get no status");
    }

    #[test]
    fn dirty_includes_untracked_and_excludes_ignored() {
        let (tmp, ctx) = fixture();
        let dest = tmp.path().join("wts").join("feat");
        add_worktree(&ctx, &dest, "feat");

        // Clean right after creation.
        let infos = list(&ctx, &status_opts()).unwrap();
        assert!(!entry(&infos, "feat").status.as_ref().unwrap().dirty);

        // Ignored files do not count as dirty.
        fs::write(dest.join(".gitignore"), "ignored.txt\n").unwrap();
        git(&dest, &["add", ".gitignore"]);
        git(&dest, &["commit", "-m", "gitignore"]);
        fs::write(dest.join("ignored.txt"), "x").unwrap();
        let infos = list(&ctx, &status_opts()).unwrap();
        assert!(!entry(&infos, "feat").status.as_ref().unwrap().dirty);

        // An untracked file does.
        fs::write(dest.join("untracked.txt"), "x").unwrap();
        let infos = list(&ctx, &status_opts()).unwrap();
        assert!(entry(&infos, "feat").status.as_ref().unwrap().dirty);
    }

    #[test]
    fn ahead_behind_with_and_without_upstream() {
        let (tmp, ctx) = fixture();
        let dest = tmp.path().join("wts").join("feat");
        add_worktree(&ctx, &dest, "feat");
        git(&dest, &["branch", "--set-upstream-to=main", "feat"]);

        // One commit on feat, one on main -> ahead 1, behind 1.
        commit_file(&dest, "feat.txt");
        commit_file(&ctx.main_root, "main.txt");

        let infos = list(&ctx, &status_opts()).unwrap();
        let feat = entry(&infos, "feat").status.as_ref().unwrap();
        assert_eq!(feat.ahead, Some(1));
        assert_eq!(feat.behind, Some(1));
        assert!(!feat.upstream_gone);

        // Main has no upstream -> both None.
        let main = entry(&infos, "main").status.as_ref().unwrap();
        assert_eq!(main.ahead, None);
        assert_eq!(main.behind, None);
        assert!(!main.upstream_gone);
    }

    #[test]
    fn upstream_gone_when_config_remains_but_ref_deleted() {
        let (tmp, ctx) = fixture();
        git(&ctx.main_root, &["branch", "tmp-up"]);
        let dest = tmp.path().join("wts").join("feat");
        add_worktree(&ctx, &dest, "feat");
        git(&dest, &["branch", "--set-upstream-to=tmp-up", "feat"]);
        git(&ctx.main_root, &["branch", "-D", "tmp-up"]);

        let infos = list(&ctx, &status_opts()).unwrap();
        let feat = entry(&infos, "feat").status.as_ref().unwrap();
        assert!(feat.upstream_gone);
        assert_eq!(feat.ahead, None);
        assert_eq!(feat.behind, None);
    }

    #[test]
    fn merged_against_default_base_main_head() {
        let (tmp, ctx) = fixture();
        // "done" points at main's tip -> merged.
        let done = tmp.path().join("wts").join("done");
        add_worktree(&ctx, &done, "done");
        // "wip" has an extra commit -> not merged.
        let wip = tmp.path().join("wts").join("wip");
        add_worktree(&ctx, &wip, "wip");
        commit_file(&wip, "wip.txt");

        let infos = list(&ctx, &status_opts()).unwrap();
        assert!(entry(&infos, "done").status.as_ref().unwrap().merged);
        assert!(!entry(&infos, "wip").status.as_ref().unwrap().merged);
        // The base's own worktree (branch == base shorthand) is never merged.
        assert!(!entry(&infos, "main").status.as_ref().unwrap().merged);
    }

    #[test]
    fn merged_with_explicit_and_unresolvable_base() {
        let (tmp, ctx) = fixture();
        let done = tmp.path().join("wts").join("done");
        add_worktree(&ctx, &done, "done");

        // Explicit base "main".
        let opts = ListOptions {
            with_status: true,
            base: Some("main".to_string()),
        };
        let infos = list(&ctx, &opts).unwrap();
        assert!(entry(&infos, "done").status.as_ref().unwrap().merged);
        assert!(!entry(&infos, "main").status.as_ref().unwrap().merged);

        // Unresolvable base falls back to main HEAD.
        let opts = ListOptions {
            with_status: true,
            base: Some("no/such/ref".to_string()),
        };
        let infos = list(&ctx, &opts).unwrap();
        assert!(entry(&infos, "done").status.as_ref().unwrap().merged);
    }

    #[test]
    fn find_resolves_name_branch_and_unique_substring() {
        let (tmp, ctx) = fixture();
        add_worktree(&ctx, &tmp.path().join("wts").join("feat-a"), "feat-a");
        add_worktree(&ctx, &tmp.path().join("wts").join("feat-b"), "feat-b");

        assert!(find(&ctx, "main").unwrap().is_main);
        assert_eq!(find(&ctx, "feat-a").unwrap().name, "feat-a");
        // Unique substring.
        assert_eq!(find(&ctx, "t-b").unwrap().name, "feat-b");
        // Resolution never computes status.
        assert!(find(&ctx, "feat-a").unwrap().status.is_none());
    }

    #[test]
    fn find_rejects_ambiguous_and_unknown_names() {
        let (tmp, ctx) = fixture();
        add_worktree(&ctx, &tmp.path().join("wts").join("feat-a"), "feat-a");
        add_worktree(&ctx, &tmp.path().join("wts").join("feat-b"), "feat-b");

        let err = find(&ctx, "feat").unwrap_err();
        match err {
            Error::WorktreeNotFound(msg) => {
                assert!(msg.contains("feat-a") && msg.contains("feat-b"), "{msg}");
            }
            other => panic!("expected WorktreeNotFound, got {other}"),
        }

        assert!(matches!(
            find(&ctx, "zzz").unwrap_err(),
            Error::WorktreeNotFound(_)
        ));
    }

    #[test]
    fn containing_maps_paths_to_worktrees() {
        let (tmp, ctx) = fixture();
        let dest = tmp.path().join("wts").join("feat");
        add_worktree(&ctx, &dest, "feat");
        let sub = dest.join("src").join("deep");
        fs::create_dir_all(&sub).unwrap();

        assert_eq!(containing(&ctx, &sub).unwrap().unwrap().name, "feat");
        assert_eq!(
            containing(&ctx, &ctx.main_root.join("a"))
                .unwrap()
                .unwrap()
                .name,
            "main"
        );
        assert!(containing(&ctx, tmp.path()).unwrap().is_none());
    }
}

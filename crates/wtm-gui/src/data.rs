//! The bridge between the window and the `wtm` library.
//!
//! Every function here is blocking and calls straight into the shared cores
//! (`wtm::worktree`, `wtm::commands::*`). None of it may run on the main
//! thread: `git2` status computation walks the working tree, and creating a
//! worktree shells out to `git`. Views call these through
//! [`gpui::AppContext::background_spawn`] and apply the result back on the
//! foreground.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use wtm::commands::{add, open, prune, remove};
use wtm::config::{self, Config};
use wtm::gitcmd;
use wtm::model::WorktreeInfo;
use wtm::repo::{self, RepoContext};
use wtm::setup::SetupEvent;
use wtm::worktree::{self, ListOptions, WorktreeDetails};

/// A repository the app has open: its resolved context plus the merged
/// configuration for it. Both are cheap value types, so this is cloned into
/// background tasks freely.
#[derive(Clone)]
pub struct OpenRepo {
    pub ctx: RepoContext,
    pub config: Config,
}

impl OpenRepo {
    /// Display name of the repository (its main working tree's directory).
    pub fn name(&self) -> &str {
        &self.ctx.repo_name
    }

    /// Absolute path of the main working tree.
    pub fn path(&self) -> &Path {
        &self.ctx.main_root
    }
}

/// Resolve `path` to a repository and load its configuration.
///
/// `path` may point anywhere inside the repo — including a linked worktree or
/// a subdirectory — and still resolves to the main working tree, which is what
/// the registry keys on.
pub fn open_repo(path: &Path) -> Result<OpenRepo, String> {
    let ctx = repo::discover(Some(path)).map_err(|e| e.to_string())?;
    let config = config::load(&ctx.main_root).map_err(|e| e.to_string())?;
    Ok(OpenRepo { ctx, config })
}

/// Resolve the repository containing the current working directory, if any.
/// Used when the app is launched from a terminal inside a repo.
pub fn open_repo_from_cwd() -> Option<OpenRepo> {
    let cwd = std::env::current_dir().ok()?;
    open_repo(&cwd).ok()
}

/// List worktrees for a repository.
///
/// `with_status` drives the expensive part (dirty/ahead/behind/merged). The
/// app loads without status first so the table paints immediately, then loads
/// again with status — the same two-pass strategy the TUI uses.
pub fn list_worktrees(repo: &OpenRepo, with_status: bool) -> Result<Vec<WorktreeInfo>, String> {
    let base = if with_status {
        repo.config.default_base.clone()
    } else {
        None
    };
    worktree::list(&repo.ctx, &ListOptions { with_status, base }).map_err(|e| e.to_string())
}

/// Detail-pane data (recent commits, upstream, remote) for one worktree.
pub fn worktree_details(path: &Path) -> Option<WorktreeDetails> {
    worktree::details(path)
}

/// Create a worktree for `branch`, reporting post-create setup progress
/// through `sink` as it happens instead of running it silently — for a
/// create-dialog progress view that has to show something during a slow
/// `npm install` rather than freezing until it finishes.
pub fn create_worktree_streaming(
    repo: &OpenRepo,
    branch: &str,
    base: Option<&str>,
    run_setup: bool,
    sink: &mut dyn FnMut(SetupEvent),
) -> Result<PathBuf, String> {
    let request = add::CreateRequest {
        branch,
        base_override: base,
        path_override: None,
        cd: false,
        run_setup,
        announce: false,
        quiet: true,
        verbose: false,
    };
    add::create_streaming(&repo.ctx, &repo.config, &request, sink).map_err(|e| e.to_string())
}

/// Remove a worktree through the safety-checked core (main worktree, cwd, and
/// dirty checks all still apply).
pub fn remove_worktree(repo: &OpenRepo, info: &WorktreeInfo, force: bool) -> Result<(), String> {
    remove::remove_worktree(&repo.ctx, info, force, true).map_err(|e| e.to_string())
}

/// Which worktrees `prune` would sweep, given the repo's protected branches.
pub fn prune_candidates(
    repo: &OpenRepo,
    rows: Vec<WorktreeInfo>,
    merged: bool,
    gone: bool,
) -> Vec<prune::PruneCandidate> {
    prune::candidates(
        rows,
        &repo.config.prune.protected_branches,
        merged,
        gone,
        false,
    )
}

/// Prune candidates the user has explicitly selected in the table.
pub fn selection_candidates(
    repo: &OpenRepo,
    rows: Vec<WorktreeInfo>,
) -> Vec<prune::PruneCandidate> {
    prune::selection_candidates(rows, &repo.config.prune.protected_branches)
}

/// Execute a prune over already-confirmed candidates.
pub fn run_prune(
    repo: &OpenRepo,
    candidates: &[prune::PruneCandidate],
    force: bool,
) -> prune::PruneReport {
    prune::execute(&repo.ctx, candidates, force, false)
}

/// Open a worktree in the configured editor (config `editor` > `$VISUAL` >
/// `$EDITOR`). This is what activating a row in the app does.
pub fn open_in_editor(repo: &OpenRepo, path: &Path) -> Result<(), String> {
    open::spawn_editor(&repo.config, path).map_err(|e| e.to_string())
}

/// Reveal a path in Finder. Both config files the Settings sheet offers to
/// reveal (`~/.config/wtm/config.toml`, `<repo>/.worktree.toml`) are
/// optional, so `path` not existing is the common case, not an edge case:
/// `open -R` exits 1 for a path that isn't there. When `path` doesn't exist,
/// walk up to the nearest existing ancestor directory and reveal that
/// instead, so the user lands in the folder where the file would go rather
/// than hitting an opaque Finder failure.
pub fn reveal_in_finder(path: &Path) -> Result<(), String> {
    let Some(target) = existing_ancestor(path) else {
        // Only possible for a relative path with no existing prefix at all
        // (an absolute path always bottoms out at a filesystem root).
        return Err(format!(
            "cannot reveal '{}' in Finder: neither it nor any parent directory exists",
            path.display()
        ));
    };
    let redirected = target.as_path() != path;

    std::process::Command::new("open")
        .arg("-R")
        .arg(&target)
        .status()
        .map_err(|e| format!("could not launch Finder: {e}"))
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else if redirected {
                Err(format!(
                    "'{}' does not exist yet; revealing '{}' instead also failed: Finder exited with {status}",
                    path.display(),
                    target.display()
                ))
            } else {
                Err(format!("Finder exited with {status}"))
            }
        })
}

/// The nearest existing ancestor of `path`: `path` itself if it already
/// exists, otherwise the first parent directory that does. `None` only when
/// no ancestor exists either. Pure path logic split out of
/// `reveal_in_finder` so it can be unit tested without spawning Finder.
fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return Some(path.to_path_buf());
    }
    path.ancestors()
        .skip(1)
        .find(|p| p.exists())
        .map(Path::to_path_buf)
}

/// Open a worktree in a terminal app. macOS only for now: `$WTM_TERMINAL`
/// names the app (`open -a <app> <path>`), falling back to `Terminal`.
pub fn open_in_terminal(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let terminal = std::env::var("WTM_TERMINAL").unwrap_or_else(|_| "Terminal".to_string());
        std::process::Command::new("open")
            .arg("-a")
            .arg(&terminal)
            .arg(path)
            .status()
            .map_err(|e| format!("could not launch {terminal}: {e}"))
            .and_then(|status| {
                if status.success() {
                    Ok(())
                } else {
                    Err(format!("{terminal} exited with {status}"))
                }
            })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err("opening a terminal is not supported on this platform yet".to_string())
    }
}

/// Copy `text` to the system clipboard: `pbcopy` on macOS; elsewhere the
/// first available of `wl-copy`, `xclip -selection clipboard`, `xsel -ib`
/// (same fallback chain as `wtm::tui`'s clipboard action, minus its OSC 52
/// terminal escape-sequence fallback — there is no terminal here to write
/// one into).
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let tools: &[(&str, &[&str])] = &[("pbcopy", &[])];
    #[cfg(not(target_os = "macos"))]
    let tools: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["-ib"]),
    ];

    for (tool, args) in tools {
        let child = std::process::Command::new(tool)
            .args(*args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            continue; // Not installed; try the next tool.
        };
        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            if stdin.write_all(text.as_bytes()).is_err() {
                continue;
            }
        }
        drop(child.stdin.take());
        match child.wait() {
            Ok(status) if status.success() => return Ok(()),
            _ => continue,
        }
    }

    let tried: Vec<&str> = tools.iter().map(|(tool, _)| *tool).collect();
    Err(format!(
        "no clipboard tool available (tried: {})",
        tried.join(", ")
    ))
}

/// One branch as shown in the create-worktree dialog's branch picker.
#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub name: String,
    pub is_local: bool,
    /// Already checked out in some worktree of this repository (a `wtm add`
    /// for it would fail with `BranchInUse`).
    pub is_checked_out: bool,
    /// Local branches only: has upstream configuration but the upstream ref
    /// no longer exists (same semantics as `WorktreeStatus::upstream_gone`).
    pub upstream_gone: bool,
}

/// Branches available to create a worktree from: local branches first
/// (alphabetical), then remote-tracking branches (alphabetical, remote
/// prefix stripped, `<remote>/HEAD` excluded, and any name already covered
/// by a local branch or another remote removed).
pub fn list_branches(repo: &OpenRepo) -> Result<Vec<BranchInfo>, String> {
    let git_repo = repo.ctx.open_main().map_err(|e| e.to_string())?;

    // Branches checked out in some worktree right now, so the picker can
    // flag entries that `wtm add` would refuse with `BranchInUse`.
    let checked_out: HashSet<String> = worktree::list(
        &repo.ctx,
        &ListOptions {
            with_status: false,
            base: None,
        },
    )
    .map_err(|e| e.to_string())?
    .into_iter()
    .filter_map(|info| info.branch)
    .collect();

    let mut local_names: HashSet<String> = HashSet::new();
    let mut locals: Vec<BranchInfo> = Vec::new();
    let local_branches = git_repo
        .branches(Some(git2::BranchType::Local))
        .map_err(|e| e.to_string())?;
    for entry in local_branches {
        let (branch, _) = entry.map_err(|e| e.to_string())?;
        let Some(name) = branch.name().map_err(|e| e.to_string())?.map(str::to_owned) else {
            continue; // Not a valid UTF-8 ref name; nothing sane to show.
        };
        let upstream_gone = match branch.upstream() {
            Ok(_) => false,
            Err(e) if e.code() == git2::ErrorCode::NotFound => git_repo
                .config()
                .and_then(|cfg| cfg.get_string(&format!("branch.{name}.merge")))
                .is_ok(),
            Err(_) => false,
        };
        local_names.insert(name.clone());
        locals.push(BranchInfo {
            is_checked_out: checked_out.contains(&name),
            name,
            is_local: true,
            upstream_gone,
        });
    }
    locals.sort_by(|a, b| a.name.cmp(&b.name));

    let mut remotes: Vec<BranchInfo> = Vec::new();
    let remote_branches = git_repo
        .branches(Some(git2::BranchType::Remote))
        .map_err(|e| e.to_string())?;
    for entry in remote_branches {
        let (branch, _) = entry.map_err(|e| e.to_string())?;
        let Some(full_name) = branch.name().map_err(|e| e.to_string())?.map(str::to_owned) else {
            continue;
        };
        let Some((_remote, short)) = full_name.split_once('/') else {
            continue; // Not `<remote>/<name>` shaped; skip rather than guess.
        };
        if short == "HEAD" {
            continue; // The remote's symbolic HEAD pointer, not a real branch.
        }
        if local_names.contains(short) {
            continue; // Already represented by its local branch.
        }
        remotes.push(BranchInfo {
            name: short.to_string(),
            is_local: false,
            is_checked_out: checked_out.contains(short),
            upstream_gone: false,
        });
    }
    remotes.sort_by(|a, b| a.name.cmp(&b.name));
    // Two remotes tracking the same branch name (e.g. origin/main and
    // upstream/main) collapse to one entry now that the remote prefix is
    // gone.
    remotes.dedup_by(|a, b| a.name == b.name);

    locals.extend(remotes);
    // The two loops above already build `locals` and `remotes` separately —
    // each internally alphabetical — before this concatenates them, so
    // locals already precede remotes. Re-deriving that ordering here from
    // `is_local` (a stable sort, so it only ever reorders across the
    // local/remote boundary, never within either group) ties it directly to
    // the field instead of leaving it as an implicit consequence of loop
    // order, so a future refactor that merges the two loops cannot silently
    // interleave local and remote branches.
    locals.sort_by_key(|b| !b.is_local);
    Ok(locals)
}

/// Delete a local branch, refusing the repo's protected branches (mirrors
/// `wtm prune`'s and `wtm rm --with-branch`'s safety rule).
pub fn delete_branch(repo: &OpenRepo, branch: &str) -> Result<(), String> {
    if repo
        .config
        .prune
        .protected_branches
        .iter()
        .any(|p| p == branch)
    {
        return Err(format!(
            "branch '{branch}' is protected and will not be touched"
        ));
    }
    gitcmd::branch_delete(&repo.ctx.main_root, branch).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------
// Base-ref picker (new-worktree dialog)
// ---------------------------------------------------------------------

/// What a [`RefInfo`] represents, so a base-ref picker can label it instead
/// of showing a flat list of names.
// Consumed by the New Worktree dialog's base-ref picker, which isn't wired
// up to this yet.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefKind {
    /// Checked out in the worktree the user is currently looking at.
    Current,
    /// The repo's configured default base (`config.default_base`), or HEAD
    /// when unset — what `wtm add` uses when the caller doesn't override the
    /// base (see `commands::add::resolve_base`).
    Default,
    /// Checked out in some other worktree — `wtm add` would refuse it.
    Worktree,
    /// An ordinary local branch.
    Local,
    /// A remote-tracking branch; `remote` names which remote.
    Remote { remote: String },
}

/// One ref as shown in the create-worktree dialog's base-ref picker.
// Consumed by the New Worktree dialog's base-ref picker, which isn't wired
// up to this yet.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefInfo {
    /// What to pass to git as the base (e.g. `main`, `origin/feature/x`).
    pub name: String,
    /// What to show in the picker (may equal `name`).
    pub display: String,
    pub kind: RefKind,
    /// Commit subject + short sha for a secondary line, if cheap to get.
    pub subject: Option<String>,
    pub short_id: Option<String>,
}

/// Refs available to branch a new worktree from, for the create-worktree
/// dialog's base-ref picker: local branches, remote-tracking branches, and
/// two synthetic markers (`Current`, `Default`) called out on whichever
/// entries carry that meaning. Unlike [`list_branches`], a local branch and
/// its remote-tracking counterpart are kept as separate entries — branching
/// from `origin/main` vs local `main` is a real distinction here.
///
/// `current_worktree` is the worktree the dialog was opened from, if known;
/// its checked-out branch (if any) becomes the `Current` entry.
// Consumed by the New Worktree dialog's base-ref picker, which isn't wired
// up to this yet.
#[allow(dead_code)]
pub fn list_refs(repo: &OpenRepo, current_worktree: Option<&Path>) -> Result<Vec<RefInfo>, String> {
    let git_repo = repo.ctx.open_main().map_err(|e| e.to_string())?;

    // One `worktree::list` call tells us both which branch is checked out
    // in `current_worktree` and which branches are checked out in some
    // OTHER worktree (`wtm add` would refuse those — `RefKind::Worktree`).
    let worktrees = worktree::list(
        &repo.ctx,
        &ListOptions {
            with_status: false,
            base: None,
        },
    )
    .map_err(|e| e.to_string())?;
    let current_canon =
        current_worktree.map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    let mut current_branch: Option<String> = None;
    let mut checked_out_elsewhere: HashSet<String> = HashSet::new();
    for w in &worktrees {
        if current_canon.as_deref() == Some(w.path.as_path()) {
            current_branch = w.branch.clone();
        } else if let Some(name) = &w.branch {
            checked_out_elsewhere.insert(name.clone());
        }
    }

    let mut refs: Vec<RawRef> = Vec::new();
    for entry in git_repo
        .branches(Some(git2::BranchType::Local))
        .map_err(|e| e.to_string())?
    {
        let (branch, _) = entry.map_err(|e| e.to_string())?;
        let Some(name) = branch.name().map_err(|e| e.to_string())?.map(str::to_owned) else {
            continue; // Not a valid UTF-8 ref name; nothing sane to show.
        };
        refs.push(RawRef { name, remote: None });
    }
    for entry in git_repo
        .branches(Some(git2::BranchType::Remote))
        .map_err(|e| e.to_string())?
    {
        let (branch, _) = entry.map_err(|e| e.to_string())?;
        let Some(full_name) = branch.name().map_err(|e| e.to_string())?.map(str::to_owned) else {
            continue;
        };
        let Some((remote, short)) = full_name.split_once('/') else {
            continue; // Not `<remote>/<name>` shaped; skip rather than guess.
        };
        if short == "HEAD" {
            continue; // The remote's symbolic HEAD pointer, not a real branch.
        }
        refs.push(RawRef {
            name: format!("{remote}/{short}"),
            remote: Some(remote.to_string()),
        });
    }

    let default_base = repo.config.default_base.as_deref();
    let mut ordered = assemble_ref_order(
        refs,
        current_branch.as_deref(),
        &checked_out_elsewhere,
        default_base,
    );

    // Subject + short id are best-effort dressing, not identity: a ref that
    // fails to resolve (e.g. the synthetic "HEAD" entry on a repo with no
    // commits yet) still keeps its place in the list.
    for r in &mut ordered {
        let (subject, short_id) = commit_info(&git_repo, &r.name);
        r.subject = subject;
        r.short_id = short_id;
    }

    Ok(ordered)
}

/// One candidate ref gathered from git, before kind-assignment and ordering.
/// Deliberately doesn't touch git2 so [`assemble_ref_order`] can be unit
/// tested without a real repository.
struct RawRef {
    /// Local: the branch name. Remote: `<remote>/<short>`.
    name: String,
    /// `None` for a local branch, `Some(remote)` for a remote-tracking one.
    remote: Option<String>,
}

/// A `RefInfo` with only `name`/`display`/`kind` filled in — what
/// [`assemble_ref_order`] can produce without touching git.
fn bare_ref(name: String, kind: RefKind) -> RefInfo {
    RefInfo {
        display: name.clone(),
        name,
        kind,
        subject: None,
        short_id: None,
    }
}

/// Order and kind-assign a flat set of refs into picker order: Current
/// first, then Default, then other local branches (alphabetical), then
/// remote-tracking branches (alphabetical).
///
/// A ref is Current when it matches `current_branch`; that check runs
/// before the Default check, so a branch that is both the current one AND
/// the configured default becomes a single `Current` entry rather than
/// showing up twice. Only once nothing local/remote matches `default_base`
/// (or `current_branch` already claimed it) does a synthetic Default entry
/// get created — this is the common case, since an unset `default_base`
/// resolves to the literal name `"HEAD"`, which is never itself a branch.
///
/// Pure: no git access, so this is unit tested directly. [`list_refs`] is
/// the git-facing wrapper that gathers `refs`, resolves `subject`/`short_id`
/// per entry, and calls this.
fn assemble_ref_order(
    refs: Vec<RawRef>,
    current_branch: Option<&str>,
    checked_out_elsewhere: &HashSet<String>,
    default_base: Option<&str>,
) -> Vec<RefInfo> {
    let default_name = default_base.unwrap_or("HEAD");

    let mut current_entry: Option<RefInfo> = None;
    let mut default_entry: Option<RefInfo> = None;
    let mut other_locals: Vec<RefInfo> = Vec::new();
    let mut other_remotes: Vec<RefInfo> = Vec::new();

    for r in refs {
        match r.remote {
            None if current_branch == Some(r.name.as_str()) => {
                current_entry = Some(bare_ref(r.name, RefKind::Current));
            }
            None if default_entry.is_none() && r.name == default_name => {
                default_entry = Some(bare_ref(r.name, RefKind::Default));
            }
            None => {
                let kind = if checked_out_elsewhere.contains(&r.name) {
                    RefKind::Worktree
                } else {
                    RefKind::Local
                };
                other_locals.push(bare_ref(r.name, kind));
            }
            Some(_) if default_entry.is_none() && r.name == default_name => {
                default_entry = Some(bare_ref(r.name, RefKind::Default));
            }
            Some(remote) => {
                other_remotes.push(bare_ref(r.name, RefKind::Remote { remote }));
            }
        }
    }

    // Nothing matched an existing ref — synthesize the Default entry,
    // unless it would just duplicate the Current row (see doc comment).
    let current_is_default = current_entry
        .as_ref()
        .is_some_and(|c| c.name == default_name);
    if default_entry.is_none() && !current_is_default {
        default_entry = Some(bare_ref(default_name.to_string(), RefKind::Default));
    }

    other_locals.sort_by(|a, b| a.name.cmp(&b.name));
    other_remotes.sort_by(|a, b| a.name.cmp(&b.name));

    let mut result = Vec::with_capacity(other_locals.len() + other_remotes.len() + 2);
    result.extend(current_entry);
    result.extend(default_entry);
    result.extend(other_locals);
    result.extend(other_remotes);
    result
}

/// Best-effort commit subject + short id for `refname`, resolved via
/// `revparse_single`. `(None, None)` for anything that doesn't resolve to a
/// commit (unborn HEAD, a stale ref, ...) rather than failing the listing —
/// these fields are secondary-line dressing for the picker, not identity.
fn commit_info(repo: &git2::Repository, refname: &str) -> (Option<String>, Option<String>) {
    let Ok(obj) = repo.revparse_single(refname) else {
        return (None, None);
    };
    let Ok(commit) = obj.peel_to_commit() else {
        return (None, None);
    };
    let subject = commit.summary().ok().flatten().map(str::to_owned);
    let short_id = commit
        .as_object()
        .short_id()
        .ok()
        .and_then(|buf| buf.as_str().ok().map(str::to_owned));
    (subject, short_id)
}

// ---------------------------------------------------------------------
// File browser
// ---------------------------------------------------------------------

/// Git status for one path in [`list_files`]/[`worktree_diff`], collapsing
/// git2's much larger `Status`/`Delta` bitflags down to what a file browser
/// or diff viewer needs to badge an entry with.
// Consumed by the worktree file browser and inline diff viewer, neither of
// which is wired up to this yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

/// One entry in a [`list_files`] directory listing.
// Consumed by the worktree file browser, which isn't wired up to this yet.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    /// Path relative to the worktree root.
    pub rel_path: PathBuf,
    pub is_dir: bool,
    /// Git status for this path, if any: modified / added / deleted /
    /// untracked / conflicted.
    pub status: Option<FileStatus>,
}

/// List one directory level inside `worktree` (not recursive — a file
/// browser expands lazily; walking the whole tree up front is exactly what
/// makes that slow on a big repo).
// Consumed by the worktree file browser, which isn't wired up to this yet.
#[allow(dead_code)]
pub fn list_files(worktree: &Path, rel_dir: &Path) -> Result<Vec<FileEntry>, String> {
    if escapes_worktree(rel_dir) {
        return Err(format!("'{}' escapes the worktree", rel_dir.display()));
    }

    let repo = git2::Repository::open(worktree).map_err(|e| e.to_string())?;
    let dir = worktree.join(rel_dir);

    // One `statuses` call for the whole worktree, indexed by path below —
    // not one call per file, which would be O(n) status walks for an
    // n-entry directory.
    let mut status_opts = git2::StatusOptions::new();
    status_opts
        .include_untracked(true)
        // A wholly-new directory then shows up as ONE status entry (path
        // ending in `/`) instead of one per file inside it, which is
        // exactly the granularity a one-level-at-a-time browser wants (we
        // don't display those nested files until the user expands the
        // directory) and is cheaper besides.
        .recurse_untracked_dirs(false)
        .include_ignored(false)
        .exclude_submodules(true);
    let statuses = repo
        .statuses(Some(&mut status_opts))
        .map_err(|e| e.to_string())?;
    let status_by_path: HashMap<String, FileStatus> = statuses
        .iter()
        .filter_map(|e| {
            let path = e.path().ok()?.to_string();
            let status = file_status_from_git(e.status())?;
            Some((path, status))
        })
        .collect();

    let read_dir =
        std::fs::read_dir(&dir).map_err(|e| format!("could not list '{}': {e}", dir.display()))?;

    let mut entries = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        let is_dir = file_type.is_dir();
        let rel_path = rel_dir.join(&name);

        // Respect .gitignore: an ignored `node_modules`/`target` full of
        // noise would otherwise make the browser useless. libgit2 matches
        // directory-only ignore patterns (e.g. `target/`) against a path
        // that ends in a slash, so directories are probed with one added.
        let mut ignore_probe = rel_path.to_string_lossy().into_owned();
        if is_dir {
            ignore_probe.push('/');
        }
        if repo
            .status_should_ignore(Path::new(&ignore_probe))
            .unwrap_or(false)
        {
            continue;
        }

        let status = status_by_path
            .get(&rel_path.to_string_lossy().into_owned())
            .copied();

        entries.push(FileEntry {
            name,
            rel_path,
            is_dir,
            status,
        });
    }

    sort_file_entries(&mut entries);
    Ok(entries)
}

/// Rejects a relative path that could climb out of the worktree via a `..`
/// component. `list_files`/`file_diff` join this straight onto the worktree
/// root, so letting `..` through would let a caller read/diff anything on
/// disk, not just inside the worktree — rejected regardless of whether a
/// `..` would net out back inside, since that's simpler to reason about
/// (and get right) than resolving the path first.
fn escapes_worktree(rel: &Path) -> bool {
    rel.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// Directories first, then files, each alphabetical case-insensitively.
/// Pure sort logic pulled out of `list_files` so it can be unit tested
/// without touching git or the filesystem.
fn sort_file_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        (!a.is_dir, a.name.to_lowercase()).cmp(&(!b.is_dir, b.name.to_lowercase()))
    });
}

/// Collapse git2's per-side (index vs. working tree) status bits into one
/// [`FileStatus`]. Priority: a conflict always wins (it blocks everything
/// else); an untracked working-tree file is next (it has no index side to
/// also be "modified"); then renamed/deleted/added; anything left still
/// carrying a modified/typechange bit falls back to `Modified`. `None` when
/// none of the bits above are set (e.g. the entry is only ignored, which
/// `list_files`/`worktree_diff` already exclude upstream of this).
fn file_status_from_git(status: git2::Status) -> Option<FileStatus> {
    if status.is_conflicted() {
        return Some(FileStatus::Conflicted);
    }
    if status.is_wt_new() {
        return Some(FileStatus::Untracked);
    }
    if status.is_index_renamed() || status.is_wt_renamed() {
        return Some(FileStatus::Renamed);
    }
    if status.is_index_deleted() || status.is_wt_deleted() {
        return Some(FileStatus::Deleted);
    }
    if status.is_index_new() {
        return Some(FileStatus::Added);
    }
    if status.is_index_modified()
        || status.is_wt_modified()
        || status.is_index_typechange()
        || status.is_wt_typechange()
    {
        return Some(FileStatus::Modified);
    }
    None
}

// ---------------------------------------------------------------------
// Diff viewer
// ---------------------------------------------------------------------

/// What kind of line one [`DiffLine`] is, mirroring `git diff`'s ` `/`+`/`-`
/// origin markers.
// Consumed by the inline diff viewer, which isn't wired up to this yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

/// One line of a [`DiffHunk`].
// Consumed by the inline diff viewer, which isn't wired up to this yet.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
}

/// One `@@ ... @@` hunk of a [`FileDiff`].
// Consumed by the inline diff viewer, which isn't wired up to this yet.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// All hunks for one file's uncommitted changes.
// Consumed by the inline diff viewer, which isn't wired up to this yet.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub status: FileStatus,
    pub hunks: Vec<DiffHunk>,
    pub binary: bool,
    /// `true` when [`MAX_DIFF_LINES_PER_FILE`] cut this file's hunks short
    /// — the UI should say so rather than silently rendering a partial diff
    /// as if it were the whole thing.
    pub truncated: bool,
}

/// Cap on diff lines collected per file (summed across all its hunks). A
/// diff of a huge generated/vendored file could otherwise be enormous; past
/// this, [`FileDiff::truncated`] is set instead of either dropping content
/// silently or materializing an unbounded string for the UI to render.
const MAX_DIFF_LINES_PER_FILE: usize = 2000;

/// Uncommitted changes in `worktree`: working tree + index vs. HEAD (an
/// unborn HEAD — no commits yet — diffs against an empty tree, so
/// everything shows as added, matching `git diff`'s own behavior there).
// Consumed by the inline diff viewer, which isn't wired up to this yet.
#[allow(dead_code)]
pub fn worktree_diff(worktree: &Path) -> Result<Vec<FileDiff>, String> {
    let repo = git2::Repository::open(worktree).map_err(|e| e.to_string())?;
    build_diffs(&repo, None)
}

/// The same as [`worktree_diff`], restricted to one file. `None` when
/// `rel_path` has no uncommitted changes.
// Consumed by the inline diff viewer, which isn't wired up to this yet.
#[allow(dead_code)]
pub fn file_diff(worktree: &Path, rel_path: &Path) -> Result<Option<FileDiff>, String> {
    if escapes_worktree(rel_path) {
        return Err(format!("'{}' escapes the worktree", rel_path.display()));
    }
    let repo = git2::Repository::open(worktree).map_err(|e| e.to_string())?;
    let pathspec = rel_path.to_string_lossy().into_owned();
    let diffs = build_diffs(&repo, Some(&pathspec))?;
    Ok(diffs.into_iter().next())
}

/// Build [`FileDiff`]s for `repo`'s working tree + index vs. HEAD, via
/// git2's diff API (`diff_tree_to_workdir_with_index`) rather than shelling
/// out to `git diff` and parsing text — this gets structured hunks and line
/// origins directly, which is both faster and less fragile. `only_path`,
/// when set, restricts the diff to an exact path (used by [`file_diff`]).
fn build_diffs(repo: &git2::Repository, only_path: Option<&str>) -> Result<Vec<FileDiff>, String> {
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());

    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        // Without this, untracked files get only their path recorded, not
        // their content -- no patch, no hunks, and binary detection can't
        // run. We want them to show up as all-added diffs, so the content
        // has to actually be read.
        .show_untracked_content(true)
        .context_lines(3);
    if let Some(p) = only_path {
        opts.pathspec(p).disable_pathspec_match(true);
    }

    let mut diff = repo
        .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))
        .map_err(|e| e.to_string())?;

    // Detect renames so a moved file shows as one Renamed entry instead of
    // a delete+add pair.
    let mut find_opts = git2::DiffFindOptions::new();
    find_opts.renames(true);
    diff.find_similar(Some(&mut find_opts))
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for idx in 0..diff.deltas().len() {
        let Some(delta) = diff.get_delta(idx) else {
            continue;
        };
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let status = delta_status(delta.status());

        // libgit2 only determines binary-ness once it actually reads the
        // file content, which happens lazily *inside* `git_patch_from_diff`
        // -- `delta.flags()` reads as plain text beforehand even for a
        // binary file. Build the patch first, then re-fetch the delta: the
        // diff's internal copy is updated in place by the patch call, so a
        // fresh `get_delta` (not the snapshot from before) is what actually
        // carries the `BINARY` flag. Never dump binary bytes into a UI
        // string.
        let patch = git2::Patch::from_diff(&diff, idx).map_err(|e| e.to_string())?;
        let is_binary = diff
            .get_delta(idx)
            .is_some_and(|d| d.flags().contains(git2::DiffFlags::BINARY));
        if is_binary {
            out.push(FileDiff {
                path,
                status,
                hunks: Vec::new(),
                binary: true,
                truncated: false,
            });
            continue;
        }
        let Some(patch) = patch else {
            // Not binary and no patch: nothing textual changed (e.g. a bare
            // mode change) -- an empty hunk list is the honest answer.
            out.push(FileDiff {
                path,
                status,
                hunks: Vec::new(),
                binary: false,
                truncated: false,
            });
            continue;
        };

        let mut hunks = Vec::new();
        let mut total_lines = 0usize;
        let mut truncated = false;
        for hunk_idx in 0..patch.num_hunks() {
            if truncated {
                break;
            }
            let (raw_hunk, _) = patch.hunk(hunk_idx).map_err(|e| e.to_string())?;
            let header = String::from_utf8_lossy(raw_hunk.header())
                .trim_end()
                .to_string();
            let line_count = patch
                .num_lines_in_hunk(hunk_idx)
                .map_err(|e| e.to_string())?;
            let mut lines = Vec::new();
            for line_idx in 0..line_count {
                if total_lines >= MAX_DIFF_LINES_PER_FILE {
                    truncated = true;
                    break;
                }
                let raw_line = patch
                    .line_in_hunk(hunk_idx, line_idx)
                    .map_err(|e| e.to_string())?;
                let kind = match raw_line.origin_value() {
                    git2::DiffLineType::Addition | git2::DiffLineType::AddEOFNL => {
                        DiffLineKind::Added
                    }
                    git2::DiffLineType::Deletion | git2::DiffLineType::DeleteEOFNL => {
                        DiffLineKind::Removed
                    }
                    _ => DiffLineKind::Context,
                };
                let text = String::from_utf8_lossy(raw_line.content())
                    .trim_end_matches('\n')
                    .to_string();
                lines.push(DiffLine {
                    kind,
                    text,
                    old_lineno: raw_line.old_lineno(),
                    new_lineno: raw_line.new_lineno(),
                });
                total_lines += 1;
            }
            if !lines.is_empty() {
                hunks.push(DiffHunk { header, lines });
            }
        }

        out.push(FileDiff {
            path,
            status,
            hunks,
            binary: false,
            truncated,
        });
    }

    Ok(out)
}

/// Map git2's delta status to our [`FileStatus`]. `Copied` folds into
/// `Added` (no dedicated variant); anything else unexpected in a diff of
/// changes (`Unmodified`/`Ignored`/`Unreadable`) degrades to `Modified`
/// rather than dropping the file from the result.
fn delta_status(status: git2::Delta) -> FileStatus {
    match status {
        git2::Delta::Added | git2::Delta::Copied => FileStatus::Added,
        git2::Delta::Untracked => FileStatus::Untracked,
        git2::Delta::Deleted => FileStatus::Deleted,
        git2::Delta::Renamed => FileStatus::Renamed,
        git2::Delta::Conflicted => FileStatus::Conflicted,
        _ => FileStatus::Modified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    // ---- shared git fixture helper (same pattern as worktree.rs's tests) ----

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

    fn init_repo(dir: &Path) {
        git(dir, &["init", "-b", "main"]);
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-m", "initial"]);
    }

    /// `main/` inside a fresh tmp dir, initialized as a repo with one commit.
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let main = tmp.path().join("main");
        std::fs::create_dir(&main).unwrap();
        init_repo(&main);
        (tmp, main)
    }

    /// An [`OpenRepo`] for `main`, with a default (not the host machine's
    /// real global) config — determinism matters more here than exercising
    /// `config::load`, which has its own tests.
    fn test_repo(main: &Path) -> OpenRepo {
        let ctx = repo::discover(Some(main)).unwrap();
        OpenRepo {
            ctx,
            config: Config::default(),
        }
    }

    // ---------------- Task 1: reveal_in_finder ----------------

    #[test]
    fn existing_ancestor_of_a_dir_is_itself() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(
            existing_ancestor(tmp.path()),
            Some(tmp.path().to_path_buf())
        );
    }

    #[test]
    fn existing_ancestor_of_a_file_is_itself() {
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(existing_ancestor(&file), Some(file));
    }

    #[test]
    fn existing_ancestor_of_missing_file_is_its_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let missing = tmp.path().join("does-not-exist.toml");
        assert_eq!(existing_ancestor(&missing), Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn existing_ancestor_none_when_all_ancestors_missing() {
        let path = Path::new("definitely/does/not/exist/anywhere/at/all");
        assert_eq!(existing_ancestor(path), None);
    }

    // ---------------- Task 2: list_refs ordering ----------------

    fn raw_local(name: &str) -> RawRef {
        RawRef {
            name: name.to_string(),
            remote: None,
        }
    }

    fn raw_remote(remote: &str, short: &str) -> RawRef {
        RawRef {
            name: format!("{remote}/{short}"),
            remote: Some(remote.to_string()),
        }
    }

    #[test]
    fn orders_current_default_locals_then_remotes() {
        let refs = vec![
            raw_local("zeta"),
            raw_local("alpha"),
            raw_local("beta"),
            raw_remote("origin", "main"),
            raw_remote("origin", "zeta"),
        ];
        let elsewhere = HashSet::new();
        let out = assemble_ref_order(refs, Some("beta"), &elsewhere, None);

        let names: Vec<&str> = out.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "beta",
                "HEAD",
                "alpha",
                "zeta",
                "origin/main",
                "origin/zeta"
            ]
        );
        assert_eq!(out[0].kind, RefKind::Current);
        assert_eq!(out[1].kind, RefKind::Default);
        assert_eq!(out[2].kind, RefKind::Local);
        assert_eq!(out[3].kind, RefKind::Local);
        assert_eq!(
            out[4].kind,
            RefKind::Remote {
                remote: "origin".to_string()
            }
        );
    }

    #[test]
    fn default_promotes_existing_local_branch_instead_of_duplicating() {
        let refs = vec![raw_local("main"), raw_local("feature")];
        let elsewhere = HashSet::new();
        let out = assemble_ref_order(refs, None, &elsewhere, Some("main"));

        assert_eq!(out.len(), 2, "no separate synthetic HEAD entry: {out:?}");
        assert_eq!(out[0].name, "main");
        assert_eq!(out[0].kind, RefKind::Default);
        assert_eq!(out[1].name, "feature");
        assert_eq!(out[1].kind, RefKind::Local);
    }

    #[test]
    fn default_promotes_existing_remote_branch_instead_of_duplicating() {
        let refs = vec![raw_local("feature"), raw_remote("origin", "main")];
        let elsewhere = HashSet::new();
        let out = assemble_ref_order(refs, None, &elsewhere, Some("origin/main"));

        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "origin/main");
        assert_eq!(out[0].kind, RefKind::Default);
        assert_eq!(out[1].name, "feature");
        assert_eq!(out[1].kind, RefKind::Local);
    }

    #[test]
    fn current_equal_to_default_yields_one_entry_not_two() {
        let refs = vec![raw_local("main")];
        let elsewhere = HashSet::new();
        let out = assemble_ref_order(refs, Some("main"), &elsewhere, Some("main"));

        assert_eq!(out.len(), 1, "expected one row, not a duplicate: {out:?}");
        assert_eq!(out[0].kind, RefKind::Current);
    }

    #[test]
    fn worktree_kind_for_branch_checked_out_elsewhere() {
        let refs = vec![raw_local("main"), raw_local("feature")];
        let mut elsewhere = HashSet::new();
        elsewhere.insert("feature".to_string());
        let out = assemble_ref_order(refs, Some("main"), &elsewhere, None);

        assert_eq!(out[0].name, "main");
        assert_eq!(out[0].kind, RefKind::Current);
        assert_eq!(out[1].name, "HEAD");
        assert_eq!(out[1].kind, RefKind::Default);
        assert_eq!(out[2].name, "feature");
        assert_eq!(out[2].kind, RefKind::Worktree);
    }

    #[test]
    fn list_refs_end_to_end_against_a_real_repo() {
        let (_tmp, main) = fixture();
        git(&main, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&main, &["branch", "feature"]);

        let repo = test_repo(&main);
        let refs = list_refs(&repo, Some(&main)).unwrap();

        let current = refs.iter().find(|r| r.name == "main").unwrap();
        assert_eq!(current.kind, RefKind::Current);
        assert!(current.subject.is_some());

        let feature = refs.iter().find(|r| r.name == "feature").unwrap();
        assert_eq!(feature.kind, RefKind::Local);

        let remote = refs.iter().find(|r| r.name == "origin/main").unwrap();
        assert_eq!(
            remote.kind,
            RefKind::Remote {
                remote: "origin".to_string()
            }
        );

        // default_base is unset -> synthesized "HEAD" entry, distinct from
        // "main" since HEAD is resolved as its own ref, not folded into the
        // branch it happens to point at.
        let default = refs.iter().find(|r| r.kind == RefKind::Default).unwrap();
        assert_eq!(default.name, "HEAD");
        assert!(default.subject.is_some());
    }

    // ---------------- Task 3: list_files ----------------

    #[test]
    fn escapes_worktree_rejects_any_parent_dir_component() {
        assert!(escapes_worktree(Path::new("../etc")));
        assert!(escapes_worktree(Path::new("a/../../b")));
        assert!(!escapes_worktree(Path::new("a/b")));
        assert!(!escapes_worktree(Path::new("")));
    }

    fn file_entry(name: &str, is_dir: bool) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            rel_path: PathBuf::from(name),
            is_dir,
            status: None,
        }
    }

    #[test]
    fn sort_file_entries_puts_dirs_first_case_insensitively() {
        let mut entries = vec![
            file_entry("zeta.txt", false),
            file_entry("Banana", true),
            file_entry("apple.txt", false),
            file_entry("alpha", true),
        ];
        sort_file_entries(&mut entries);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "Banana", "apple.txt", "zeta.txt"]);
    }

    #[test]
    fn list_files_rejects_escaping_rel_dir() {
        let (_tmp, main) = fixture();
        let err = list_files(&main, Path::new("../escape")).unwrap_err();
        assert!(err.contains("escapes"), "{err}");
    }

    #[test]
    fn list_files_respects_gitignore_and_attaches_status() {
        let (_tmp, main) = fixture();

        std::fs::write(main.join(".gitignore"), "target/\nignored.txt\n").unwrap();
        git(&main, &["add", ".gitignore"]);
        git(&main, &["commit", "-m", "gitignore"]);

        std::fs::create_dir(main.join("target")).unwrap();
        std::fs::write(main.join("target").join("build.o"), "x").unwrap();
        std::fs::write(main.join("ignored.txt"), "x").unwrap();
        std::fs::write(main.join("new.txt"), "x").unwrap();
        std::fs::create_dir(main.join("src")).unwrap();
        std::fs::write(main.join("src").join("lib.rs"), "fn a() {}\n").unwrap();
        std::fs::write(main.join("README.md"), "changed\n").unwrap();

        let entries = list_files(&main, Path::new("")).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"target"),
            "gitignored dir must not be listed: {names:?}"
        );
        assert!(
            !names.contains(&"ignored.txt"),
            "gitignored file must not be listed: {names:?}"
        );
        assert!(names.contains(&"src"));
        assert!(names.contains(&"new.txt"));

        let readme = entries.iter().find(|e| e.name == "README.md").unwrap();
        assert_eq!(readme.status, Some(FileStatus::Modified));
        let new_file = entries.iter().find(|e| e.name == "new.txt").unwrap();
        assert_eq!(new_file.status, Some(FileStatus::Untracked));

        // Directories sort before files.
        let src_pos = entries.iter().position(|e| e.name == "src").unwrap();
        let new_pos = entries.iter().position(|e| e.name == "new.txt").unwrap();
        assert!(src_pos < new_pos);
    }

    // ---------------- Task 4: worktree_diff / file_diff ----------------

    #[test]
    fn worktree_diff_reports_hunks_with_line_numbers() {
        let (_tmp, main) = fixture();
        std::fs::write(main.join("README.md"), "hello\nworld\n").unwrap();

        let diffs = worktree_diff(&main).unwrap();
        let readme = diffs.iter().find(|d| d.path == "README.md").unwrap();
        assert_eq!(readme.status, FileStatus::Modified);
        assert!(!readme.binary);
        assert_eq!(readme.hunks.len(), 1);

        let added: Vec<&DiffLine> = readme.hunks[0]
            .lines
            .iter()
            .filter(|l| l.kind == DiffLineKind::Added)
            .collect();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].text, "world");
        assert_eq!(added[0].new_lineno, Some(2));
        assert_eq!(added[0].old_lineno, None);
    }

    #[test]
    fn worktree_diff_shows_untracked_file_as_all_added() {
        let (_tmp, main) = fixture();
        std::fs::write(main.join("new.txt"), "one\ntwo\n").unwrap();

        let diffs = worktree_diff(&main).unwrap();
        let new_file = diffs.iter().find(|d| d.path == "new.txt").unwrap();
        assert_eq!(new_file.status, FileStatus::Untracked);
        assert!(!new_file.hunks.is_empty());
        assert!(new_file
            .hunks
            .iter()
            .all(|h| h.lines.iter().all(|l| l.kind == DiffLineKind::Added)));
    }

    #[test]
    fn worktree_diff_marks_binary_files_without_hunks() {
        let (_tmp, main) = fixture();
        std::fs::write(main.join("bin.dat"), [0u8, 159, 146, 150, 0, 1, 2, 3]).unwrap();

        let diffs = worktree_diff(&main).unwrap();
        let bin = diffs.iter().find(|d| d.path == "bin.dat").unwrap();
        assert!(bin.binary);
        assert!(bin.hunks.is_empty());
    }

    #[test]
    fn file_diff_restricts_to_one_file() {
        let (_tmp, main) = fixture();
        std::fs::write(main.join("README.md"), "hello\nworld\n").unwrap();
        std::fs::write(main.join("other.txt"), "x").unwrap();

        let diff = file_diff(&main, Path::new("README.md")).unwrap().unwrap();
        assert_eq!(diff.path, "README.md");

        assert!(file_diff(&main, Path::new("does-not-exist.txt"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn file_diff_rejects_escaping_rel_path() {
        let (_tmp, main) = fixture();
        let err = file_diff(&main, Path::new("../escape.txt")).unwrap_err();
        assert!(err.contains("escapes"), "{err}");
    }

    #[test]
    fn worktree_diff_truncates_huge_files() {
        let (_tmp, main) = fixture();
        let big: String = (0..3000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(main.join("big.txt"), big).unwrap();

        let diffs = worktree_diff(&main).unwrap();
        let big_file = diffs.iter().find(|d| d.path == "big.txt").unwrap();
        assert!(big_file.truncated);
        let total_lines: usize = big_file.hunks.iter().map(|h| h.lines.len()).sum();
        assert_eq!(total_lines, MAX_DIFF_LINES_PER_FILE);
    }
}

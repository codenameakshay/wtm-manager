//! `wtm prune` — sweep stale worktrees (missing/prunable, and optionally
//! merged or upstream-gone branches).
//!
//! Candidate selection (`candidates` / `selection_candidates`) and
//! execution (`execute`) are shared with the TUI and GUI, which show the same
//! would-prune list in a confirm modal before acting.

use crate::cli::{GlobalArgs, PruneArgs};
use crate::error::{Error, Result};
use crate::gitcmd;
use crate::model::WorktreeInfo;
use crate::repo::RepoContext;
use crate::worktree::{self, ListOptions};
use rayon::prelude::*;

/// One worktree selected for pruning, with why and what to do about its
/// branch.
#[derive(Debug, Clone)]
pub struct PruneCandidate {
    pub info: WorktreeInfo,
    pub reasons: Vec<&'static str>,
    /// Merged/gone candidates get their branch deleted (that is the point of
    /// pruning); missing-dir-only entries never (we only clean the registry).
    pub delete_branch: bool,
}

/// What [`execute`] did: how many worktrees were removed, and which were
/// skipped because they were dirty (and `force` was off).
pub struct PruneReport {
    /// How many worktrees were actually removed.
    pub removed: usize,
    /// Display names skipped because they were dirty and `force` was off.
    pub skipped: Vec<String>,
    /// One message per worktree that failed to prune, naming the worktree.
    pub failures: Vec<String>,
}

/// Prune stale worktrees and finish with `git worktree prune`.
pub fn run(args: &PruneArgs, global: &GlobalArgs) -> Result<()> {
    let (ctx, config) = super::prepare(global)?;

    // Status (dirty/merged/gone) is only needed when a status-derived
    // selection or safety check can trigger.
    let with_status = args.merged || args.gone || !args.force;
    let items = worktree::list(
        &ctx,
        &ListOptions {
            with_status,
            base: config.default_base.clone(),
        },
    )?;

    let candidates = candidates(
        &items,
        &config.prune.protected_branches,
        args.merged,
        args.gone,
        global.verbose,
    );

    if candidates.is_empty() {
        if !global.quiet {
            eprintln!("nothing to prune");
        }
        // Still let git tidy its registry.
        if !args.dry_run {
            gitcmd::worktree_prune(&ctx.main_root)?;
        }
        return Ok(());
    }

    if args.dry_run {
        println!("Would prune {} worktree(s):", candidates.len());
        for c in &candidates {
            println!(
                "  {} ({}) [{}]{}",
                c.info.display_name(),
                c.info.path.display(),
                c.reasons.join(", "),
                if c.delete_branch {
                    " + delete branch"
                } else {
                    ""
                }
            );
        }
        println!("Would run `git worktree prune`.");
        return Ok(());
    }

    let report = execute(&ctx, &candidates, args.force, true, &|_| {});
    if !global.quiet {
        eprintln!("pruned {} worktree(s)", report.removed);
    }
    if !report.failures.is_empty() {
        return Err(Error::Other(format!(
            "prune completed with {} failure(s): {}",
            report.failures.len(),
            report.failures.join("; ")
        )));
    }
    Ok(())
}

/// Select prune candidates from a listing: never the main worktree, never a
/// protected branch; reasons are missing/prunable always, plus merged/gone
/// when the corresponding flag is set. `verbose` prints a stderr note for
/// each protected skip (CLI only; the TUI and GUI pass false to stay pure).
pub fn candidates(
    items: &[WorktreeInfo],
    protected: &[String],
    merged: bool,
    gone: bool,
    verbose: bool,
) -> Vec<PruneCandidate> {
    let mut selected: Vec<PruneCandidate> = Vec::new();

    for info in items {
        if info.is_main {
            continue;
        }
        if let Some(branch) = &info.branch {
            if protected.iter().any(|p| p == branch) {
                if verbose {
                    eprintln!("skipping '{branch}': protected branch");
                }
                continue;
            }
        }

        let mut reasons: Vec<&'static str> = Vec::new();
        if info.is_missing {
            reasons.push("missing");
        }
        if info.is_prunable {
            reasons.push("prunable");
        }
        let status = info.status.as_ref();
        let is_merged = merged && status.is_some_and(|s| s.merged);
        if is_merged {
            reasons.push("merged");
        }
        let is_gone = gone && status.is_some_and(|s| s.upstream_gone);
        if is_gone {
            reasons.push("gone");
        }
        if reasons.is_empty() {
            continue;
        }

        let delete_branch = (is_merged || is_gone) && info.branch.is_some() && !info.is_missing;
        selected.push(PruneCandidate {
            info: info.clone(),
            reasons,
            delete_branch,
        });
    }

    selected
}

/// Candidates from an explicit user selection (TUI/GUI multi-select): protected
/// branches and the main worktree are still skipped, and branches are never
/// deleted — the user asked to remove worktrees, not branches.
pub fn selection_candidates(items: Vec<WorktreeInfo>, protected: &[String]) -> Vec<PruneCandidate> {
    items
        .into_iter()
        .filter(|info| !info.is_main)
        .filter(|info| {
            !info
                .branch
                .as_deref()
                .is_some_and(|b| protected.iter().any(|p| p == b))
        })
        .map(|info| {
            let mut reasons = vec!["selected"];
            if info.is_missing {
                reasons.push("missing");
            }
            PruneCandidate {
                info,
                reasons,
                delete_branch: false,
            }
        })
        .collect()
}

/// How many worktrees are removed concurrently. Removal is I/O bound (git's
/// own dirty scan plus deleting the tree); four workers cut a 150-worktree
/// prune from 20s to 8s on an SSD, and eight bought nothing more.
const REMOVE_PARALLELISM: usize = 4;

/// Remove every candidate (skipping dirty ones unless `force`, exactly like
/// `remove`), delete branches where flagged, and finish with
/// `git worktree prune`. `announce` prints the CLI's per-item stdout lines
/// and skip warnings; the TUI and GUI pass false and report via the returned
/// [`PruneReport`]. `progress` is called after each candidate is dealt with,
/// with the running count, so a UI can show "n of N".
///
/// Removals run `REMOVE_PARALLELISM` at a time; a removal that fails is
/// retried once sequentially, since concurrent `git worktree remove` calls
/// have been seen to trip over each other transiently. Branch deletion is one
/// `git branch -D` for every removed candidate, falling back to one call per
/// branch only to attribute a failure.
pub fn execute(
    ctx: &RepoContext,
    candidates: &[PruneCandidate],
    force: bool,
    announce: bool,
    progress: &(dyn Fn(usize) + Sync),
) -> PruneReport {
    let done = std::sync::atomic::AtomicUsize::new(0);
    let step = |candidate: &PruneCandidate| -> Outcome {
        let outcome = remove_one(ctx, candidate, force, announce);
        progress(done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1);
        outcome
    };
    let mut outcomes: Vec<Outcome> = match rayon::ThreadPoolBuilder::new()
        .num_threads(REMOVE_PARALLELISM)
        .build()
    {
        Ok(pool) => pool.install(|| candidates.par_iter().map(step).collect()),
        Err(_) => candidates.iter().map(step).collect(),
    };

    for (candidate, outcome) in candidates.iter().zip(outcomes.iter_mut()) {
        if matches!(outcome, Outcome::Failed(_)) {
            *outcome = remove_one(ctx, candidate, force, announce);
        }
    }

    let mut removed = 0usize;
    let mut skipped = Vec::new();
    let mut failures = Vec::new();
    let mut branches: Vec<&str> = Vec::new();
    for (candidate, outcome) in candidates.iter().zip(outcomes) {
        match outcome {
            Outcome::Removed => {
                removed += 1;
                if candidate.delete_branch {
                    branches.extend(candidate.info.branch.as_deref());
                }
            }
            Outcome::Skipped => skipped.push(candidate.info.display_name().to_string()),
            Outcome::Failed(failure) => {
                if announce {
                    eprintln!("warning: failed to prune {failure}");
                }
                failures.push(failure);
            }
        }
    }

    if !branches.is_empty() && gitcmd::branch_delete(&ctx.main_root, &branches).is_err() {
        for branch in &branches {
            if let Err(error) = gitcmd::branch_delete(&ctx.main_root, &[branch]) {
                let failure = format!("delete branch '{branch}': {error}");
                if announce {
                    eprintln!("warning: {failure}");
                }
                failures.push(failure);
            }
        }
    }
    if announce {
        for branch in &branches {
            if !failures.iter().any(|f| f.contains(&format!("'{branch}'"))) {
                println!("Deleted branch '{branch}'");
            }
        }
    }

    if let Err(error) = gitcmd::worktree_prune(&ctx.main_root) {
        let failure = format!("final git worktree prune: {error}");
        if announce {
            eprintln!("warning: {failure}");
        }
        failures.push(failure);
    }
    PruneReport {
        removed,
        skipped,
        failures,
    }
}

enum Outcome {
    Removed,
    Skipped,
    Failed(String),
}

/// Re-check the filesystem immediately before removal (the listing shown to
/// the user may be stale by the time they confirm; an unavailable scan fails
/// closed rather than counting as clean), then `git worktree remove`.
fn remove_one(ctx: &RepoContext, c: &PruneCandidate, force: bool, announce: bool) -> Outcome {
    if !force && !c.info.is_missing {
        match super::remove::is_dirty(&c.info.path) {
            Ok(true) => {
                if announce {
                    eprintln!(
                        "warning: skipping '{}': uncommitted changes (use --force to override)",
                        c.info.display_name()
                    );
                }
                return Outcome::Skipped;
            }
            Ok(false) => {}
            Err(error) => {
                return Outcome::Failed(format!(
                    "{}: could not verify clean state: {error}",
                    c.info.display_name()
                ));
            }
        }
    }

    // Missing dirs need --force: it is the only way git drops the entry.
    let entry_force = force || c.info.is_missing;
    if let Err(error) = gitcmd::worktree_remove(&ctx.main_root, &c.info.path, entry_force) {
        return Outcome::Failed(format!("{}: {error}", c.info.display_name()));
    }
    if announce {
        println!(
            "Removed worktree '{}' ({}) [{}]",
            c.info.display_name(),
            c.info.path.display(),
            c.reasons.join(", ")
        );
    }
    Outcome::Removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WorktreeStatus;
    use crate::testgit::{git, init_repo};

    fn candidate(path: std::path::PathBuf, name: &str) -> PruneCandidate {
        PruneCandidate {
            info: WorktreeInfo {
                name: name.to_string(),
                path,
                branch: Some(name.to_string()),
                head: None,
                is_main: false,
                is_missing: false,
                is_locked: false,
                is_prunable: true,
                // Deliberately stale "clean" status: execute must re-check.
                status: Some(WorktreeStatus {
                    dirty: false,
                    dirty_count: 0,
                    ahead: None,
                    behind: None,
                    upstream_gone: false,
                    merged: false,
                }),
            },
            reasons: vec!["prunable"],
            delete_branch: false,
        }
    }

    #[test]
    fn execute_rechecks_dirty_state_and_fails_closed_when_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("main");
        init_repo(&main);
        let dirty = temp.path().join("dirty");
        init_repo(&dirty);
        std::fs::write(dirty.join("untracked"), "changed\n").unwrap();
        let unavailable = temp.path().join("not-a-repository");
        std::fs::create_dir(&unavailable).unwrap();

        let ctx = RepoContext {
            main_root: main.clone(),
            git_dir: main.join(".git"),
            repo_name: "main".to_string(),
        };
        let report = execute(
            &ctx,
            &[
                candidate(dirty.clone(), "dirty"),
                candidate(unavailable.clone(), "unavailable"),
            ],
            false,
            false,
            &|_| {},
        );

        assert_eq!(report.removed, 0);
        assert_eq!(report.skipped, ["dirty"]);
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0].contains("could not verify clean state"));
        assert!(dirty.is_dir());
        assert!(unavailable.is_dir());
    }

    /// `execute` removes candidates on its bounded rayon pool, reports "n of
    /// N" progress in completion order (not input order), and deletes every
    /// flagged branch in one `git branch -D` call.
    #[test]
    fn execute_removes_in_parallel_reports_progress_and_deletes_branches() {
        const N: usize = 12;
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("main");
        init_repo(&main);
        let ctx = RepoContext {
            main_root: main.clone(),
            git_dir: main.join(".git"),
            repo_name: "main".to_string(),
        };

        let mut paths = Vec::with_capacity(N);
        for i in 0..N {
            let path = temp.path().join(format!("wt{i}"));
            git(
                &main,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &format!("feat/{i}"),
                    path.to_str().unwrap(),
                    "main",
                ],
            );
            paths.push(path);
        }

        let items = worktree::list(
            &ctx,
            &ListOptions {
                with_status: true,
                base: None,
            },
        )
        .unwrap();
        // Every branch shares main's tip (no commit of its own), so all N
        // are merged candidates.
        let cands = candidates(&items, &[], true, false, false);
        assert_eq!(cands.len(), N);

        let progress_log: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());
        let progress = |n: usize| progress_log.lock().unwrap().push(n);
        let report = execute(&ctx, &cands, false, false, &progress);

        assert_eq!(report.removed, N);
        assert!(report.failures.is_empty(), "{:?}", report.failures);

        let mut seen = progress_log.into_inner().unwrap();
        assert_eq!(seen.len(), N);
        assert_eq!(
            *seen.last().unwrap(),
            N,
            "the final progress call must report N"
        );
        seen.sort_unstable();
        assert_eq!(seen, (1..=N).collect::<Vec<_>>());

        for path in &paths {
            assert!(
                !path.is_dir(),
                "{} should have been removed",
                path.display()
            );
        }
        assert!(
            git(&main, &["branch", "--list", "feat/*"]).is_empty(),
            "every feat/* branch must be deleted"
        );
        assert_eq!(
            git(&main, &["worktree", "list"]).lines().count(),
            1,
            "only main should remain"
        );
    }
}

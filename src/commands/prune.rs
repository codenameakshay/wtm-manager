//! `wtm prune` — sweep stale worktrees (missing/prunable, and optionally
//! merged or upstream-gone branches).
//!
//! Candidate selection ([`candidates`] / [`selection_candidates`]) and
//! execution ([`execute`]) are shared with the TUI, which shows the same
//! would-prune list in a confirm modal before acting.

use crate::cli::{GlobalArgs, PruneArgs};
use crate::error::Result;
use crate::gitcmd;
use crate::model::WorktreeInfo;
use crate::repo::RepoContext;
use crate::worktree::{self, ListOptions};

/// One worktree selected for pruning, with why and what to do about its
/// branch.
#[derive(Debug, Clone)]
pub(crate) struct PruneCandidate {
    pub info: WorktreeInfo,
    pub reasons: Vec<&'static str>,
    /// Merged/gone candidates get their branch deleted (that is the point of
    /// pruning); missing-dir-only entries never (we only clean the registry).
    pub delete_branch: bool,
}

/// What [`execute`] did: how many worktrees were removed, and which were
/// skipped because they were dirty (and `force` was off).
pub(crate) struct PruneReport {
    pub removed: usize,
    pub skipped: Vec<String>,
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
        items,
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

    let report = execute(&ctx, &candidates, args.force, true)?;
    if !global.quiet {
        eprintln!("pruned {} worktree(s)", report.removed);
    }
    Ok(())
}

/// Select prune candidates from a listing: never the main worktree, never a
/// protected branch; reasons are missing/prunable always, plus merged/gone
/// when the corresponding flag is set. `verbose` prints a stderr note for
/// each protected skip (CLI only; the TUI passes false to stay pure).
pub(crate) fn candidates(
    items: Vec<WorktreeInfo>,
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
            info,
            reasons,
            delete_branch,
        });
    }

    selected
}

/// Candidates from an explicit user selection (TUI multi-select): protected
/// branches and the main worktree are still skipped, and branches are never
/// deleted — the user asked to remove worktrees, not branches.
pub(crate) fn selection_candidates(
    items: Vec<WorktreeInfo>,
    protected: &[String],
) -> Vec<PruneCandidate> {
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

/// Remove every candidate (skipping dirty ones unless `force`, exactly like
/// `remove`), delete branches where flagged, and finish with
/// `git worktree prune`. `announce` prints the CLI's per-item stdout lines
/// and skip warnings; the TUI passes false and reports via the returned
/// [`PruneReport`].
pub(crate) fn execute(
    ctx: &RepoContext,
    candidates: &[PruneCandidate],
    force: bool,
    announce: bool,
) -> Result<PruneReport> {
    let mut removed = 0usize;
    let mut skipped: Vec<String> = Vec::new();

    for c in candidates {
        // Dirty safety, exactly like `remove`, unless --force. Skipping (with
        // a warning) instead of aborting keeps the sweep useful.
        if !force && !c.info.is_missing && c.info.status.as_ref().is_some_and(|s| s.dirty) {
            if announce {
                eprintln!(
                    "warning: skipping '{}': uncommitted changes (use --force to override)",
                    c.info.display_name()
                );
            }
            skipped.push(c.info.display_name().to_string());
            continue;
        }

        // Missing dirs need --force: it is the only way git drops the entry.
        let entry_force = force || c.info.is_missing;
        gitcmd::worktree_remove(&ctx.main_root, &c.info.path, entry_force)?;
        if announce {
            println!(
                "Removed worktree '{}' ({}) [{}]",
                c.info.display_name(),
                c.info.path.display(),
                c.reasons.join(", ")
            );
        }
        removed += 1;

        if c.delete_branch {
            if let Some(branch) = &c.info.branch {
                gitcmd::branch_delete(&ctx.main_root, branch)?;
                if announce {
                    println!("Deleted branch '{branch}'");
                }
            }
        }
    }

    gitcmd::worktree_prune(&ctx.main_root)?;
    Ok(PruneReport { removed, skipped })
}

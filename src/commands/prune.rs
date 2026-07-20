//! `wtm prune` — sweep stale worktrees (missing/prunable, and optionally
//! merged or upstream-gone branches).

use crate::cli::{GlobalArgs, PruneArgs};
use crate::error::Result;
use crate::gitcmd;
use crate::model::WorktreeInfo;
use crate::worktree::{self, ListOptions};

/// One worktree selected for pruning, with why and what to do about its
/// branch.
struct Candidate {
    info: WorktreeInfo,
    reasons: Vec<&'static str>,
    /// Merged/gone candidates get their branch deleted (that is the point of
    /// pruning); missing-dir-only entries never (we only clean the registry).
    delete_branch: bool,
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

    let protected = &config.prune.protected_branches;
    let mut candidates: Vec<Candidate> = Vec::new();

    for info in items {
        if info.is_main {
            continue;
        }
        if let Some(branch) = &info.branch {
            if protected.iter().any(|p| p == branch) {
                if global.verbose {
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
        let merged = args.merged && status.is_some_and(|s| s.merged);
        if merged {
            reasons.push("merged");
        }
        let gone = args.gone && status.is_some_and(|s| s.upstream_gone);
        if gone {
            reasons.push("gone");
        }
        if reasons.is_empty() {
            continue;
        }

        let delete_branch = (merged || gone) && info.branch.is_some() && !info.is_missing;
        candidates.push(Candidate {
            info,
            reasons,
            delete_branch,
        });
    }

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

    let mut removed = 0usize;
    for c in &candidates {
        // Dirty safety, exactly like `remove`, unless --force. Skipping (with
        // a warning) instead of aborting keeps the sweep useful.
        if !args.force && !c.info.is_missing && c.info.status.as_ref().is_some_and(|s| s.dirty) {
            eprintln!(
                "warning: skipping '{}': uncommitted changes (use --force to override)",
                c.info.display_name()
            );
            continue;
        }

        // Missing dirs need --force: it is the only way git drops the entry.
        let force = args.force || c.info.is_missing;
        gitcmd::worktree_remove(&ctx.main_root, &c.info.path, force)?;
        println!(
            "Removed worktree '{}' ({}) [{}]",
            c.info.display_name(),
            c.info.path.display(),
            c.reasons.join(", ")
        );
        removed += 1;

        if c.delete_branch {
            if let Some(branch) = &c.info.branch {
                gitcmd::branch_delete(&ctx.main_root, branch)?;
                println!("Deleted branch '{branch}'");
            }
        }
    }

    gitcmd::worktree_prune(&ctx.main_root)?;
    if !global.quiet {
        eprintln!("pruned {removed} worktree(s)");
    }
    Ok(())
}

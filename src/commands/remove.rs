//! `wtm remove` — remove a worktree, with dirty/main/cwd safety checks.
//!
//! The safety-checked removal itself lives in the private `remove_worktree` core so that
//! the CLI command and the TUI `d` action share one implementation.

use std::path::Path;

use crate::cli::{GlobalArgs, RemoveArgs};
use crate::error::{Error, Result};
use crate::gitcmd;
use crate::model::WorktreeInfo;
use crate::repo::RepoContext;

/// Remove a worktree (picked interactively when no name is given).
pub fn run(args: &RemoveArgs, global: &GlobalArgs) -> Result<()> {
    let (ctx, config) = super::prepare(global)?;
    let target = super::resolve_target(&ctx, args.name.as_deref(), "remove")?;

    // Validate branch deletion before removing the worktree. A protected
    // branch must leave the entire worktree operation untouched.
    let branch_to_delete = if args.with_branch {
        match target.branch.as_deref() {
            Some(branch) if config.prune.protected_branches.iter().any(|p| p == branch) => {
                return Err(Error::ProtectedBranch(branch.to_string()));
            }
            Some(branch) => Some(branch.to_string()),
            None => None,
        }
    } else {
        None
    };

    remove_worktree(&ctx, &target, args.force, global.quiet)?;

    if !global.quiet {
        println!(
            "Removed worktree '{}' ({})",
            target.display_name(),
            target.path.display()
        );
    }

    match branch_to_delete {
        Some(branch) => {
            gitcmd::branch_delete(&ctx.main_root, &branch)?;
            if !global.quiet {
                println!("Deleted branch '{branch}'");
            }
        }
        None if args.with_branch && !global.quiet => {
            eprintln!("note: no branch was checked out; nothing to delete");
        }
        None => {}
    }

    Ok(())
}

/// Shared removal core with every safety rule:
/// - the main worktree is never removed;
/// - the worktree containing the current directory is never removed;
/// - a dirty worktree is refused unless `force`;
/// - a registry entry whose directory is already gone is removed with
///   `--force` (the only way git drops the stale entry; nothing on disk is
///   touched).
pub(crate) fn remove_worktree(
    ctx: &RepoContext,
    target: &WorktreeInfo,
    force: bool,
    quiet: bool,
) -> Result<()> {
    if target.is_main {
        return Err(Error::MainWorktree {
            action: "remove".to_string(),
        });
    }

    // Refuse to remove the worktree the user is standing in.
    if contains_cwd(&target.path) {
        return Err(Error::Other(format!(
            "refusing to remove '{}': it contains the current directory (cd elsewhere first)",
            target.display_name()
        )));
    }

    if target.is_missing {
        if !quiet {
            eprintln!(
                "note: directory {} is missing; removing the stale registry entry",
                target.path.display()
            );
        }
        gitcmd::worktree_remove(&ctx.main_root, &target.path, true)?;
    } else {
        if !force && is_dirty(&target.path)? {
            return Err(Error::Dirty {
                name: target.display_name().to_string(),
                path: target.path.clone(),
            });
        }
        gitcmd::worktree_remove(&ctx.main_root, &target.path, force)?;
    }

    Ok(())
}

/// Is `path` (or a subdirectory of it) the current working directory?
fn contains_cwd(path: &Path) -> bool {
    let Ok(cwd) = std::env::current_dir() else {
        return false;
    };
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    let target = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    cwd.starts_with(&target)
}

/// Uncommitted changes (including untracked, excluding ignored/submodules)?
pub(crate) fn is_dirty(path: &Path) -> Result<bool> {
    let repo = git2::Repository::open(path)?;
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .include_ignored(false)
        .exclude_submodules(true);
    let dirty = !repo.statuses(Some(&mut opts))?.is_empty();
    Ok(dirty)
}

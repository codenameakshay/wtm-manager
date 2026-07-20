//! `wtm remove` — remove a worktree, with dirty/main/cwd safety checks.

use std::path::Path;

use crate::cli::{GlobalArgs, RemoveArgs};
use crate::error::{Error, Result};
use crate::gitcmd;

/// Remove a worktree (picked interactively when no name is given).
pub fn run(args: &RemoveArgs, global: &GlobalArgs) -> Result<()> {
    let (ctx, config) = super::prepare(global)?;
    let target = super::resolve_target(&ctx, args.name.as_deref(), "remove")?;

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
        // The directory is gone; `git worktree remove --force` is the only
        // way to drop the stale registry entry. Nothing on disk is touched.
        if !global.quiet {
            eprintln!(
                "note: directory {} is missing; removing the stale registry entry",
                target.path.display()
            );
        }
        gitcmd::worktree_remove(&ctx.main_root, &target.path, true)?;
    } else {
        if !args.force && is_dirty(&target.path)? {
            return Err(Error::Dirty {
                name: target.display_name().to_string(),
                path: target.path.clone(),
            });
        }
        gitcmd::worktree_remove(&ctx.main_root, &target.path, args.force)?;
    }

    println!(
        "Removed worktree '{}' ({})",
        target.display_name(),
        target.path.display()
    );

    if args.with_branch {
        match &target.branch {
            Some(branch) => {
                if config.prune.protected_branches.iter().any(|p| p == branch) {
                    return Err(Error::ProtectedBranch(branch.clone()));
                }
                gitcmd::branch_delete(&ctx.main_root, branch)?;
                println!("Deleted branch '{branch}'");
            }
            None => {
                if !global.quiet {
                    eprintln!("note: no branch was checked out; nothing to delete");
                }
            }
        }
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
fn is_dirty(path: &Path) -> Result<bool> {
    let repo = git2::Repository::open(path)?;
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .include_ignored(false)
        .exclude_submodules(true);
    let dirty = !repo.statuses(Some(&mut opts))?.is_empty();
    Ok(dirty)
}

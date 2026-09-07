//! `wtm remove` — remove a worktree, with dirty/main/cwd safety checks.
//!
//! The safety-checked removal itself lives in `remove_worktree`, shared by
//! the CLI command, the TUI `d` action, and the GUI. The "contains cwd"
//! guard is applied by the CLI and TUI only — the GUI process cwd is not
//! the user's shell, so refusing there would block a legitimate Remove.

use std::path::Path;
#[cfg(test)]
use std::sync::Mutex;

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

    if contains_cwd(&target.path) {
        return Err(Error::Other(format!(
            "refusing to remove '{}': it contains the current directory (cd elsewhere first)",
            target.display_name()
        )));
    }

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
            gitcmd::branch_delete(&ctx.main_root, &[&branch])?;
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

/// Shared removal core with every safety rule except the shell cwd guard:
/// - the main worktree is never removed;
/// - a dirty worktree is refused unless `force`;
/// - a registry entry whose directory is already gone is removed with
///   `--force` (the only way git drops the stale entry; nothing on disk is
///   touched).
///
/// Callers that represent a user's shell (CLI `run`, TUI `d`) must apply
/// [`contains_cwd`] themselves. The GUI must not: its process cwd is not
/// the directory the user is standing in.
pub fn remove_worktree(
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
pub(crate) fn contains_cwd(path: &Path) -> bool {
    let Ok(cwd) = std::env::current_dir() else {
        return false;
    };
    let cwd = crate::repo::canonicalize_lossy(&cwd);
    let target = crate::repo::canonicalize_lossy(path);
    cwd.starts_with(&target)
}

/// Serializes tests that change process cwd so they cannot race
/// `prune::exclude_cwd` (which reads `current_dir`).
#[cfg(test)]
pub(crate) static CWD_LOCK: Mutex<()> = Mutex::new(());

/// Uncommitted changes (including untracked, excluding ignored/submodules)?
pub fn is_dirty(path: &Path) -> Result<bool> {
    let repo = git2::Repository::open(path)?;
    let dirty = !repo
        .statuses(Some(&mut crate::worktree::dirty_status_options()))?
        .is_empty();
    Ok(dirty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testgit::{git, init_repo};
    use crate::worktree;

    struct RestoreCwd(std::path::PathBuf);
    impl Drop for RestoreCwd {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    #[test]
    fn remove_worktree_does_not_refuse_process_cwd() {
        let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::TempDir::new().unwrap();
        let main = tmp.path().join("main");
        init_repo(&main);
        let dest = tmp.path().join("wt-feat");
        git(
            &main,
            &["worktree", "add", "-b", "feat", dest.to_str().unwrap()],
        );
        let ctx = crate::repo::discover(Some(&main)).unwrap();
        let target = worktree::find(&ctx, "feat").unwrap();

        let _restore = RestoreCwd(std::env::current_dir().unwrap());
        std::env::set_current_dir(&dest).unwrap();
        remove_worktree(&ctx, &target, false, true).expect(
            "GUI callers must be able to remove a worktree that happens to contain the process cwd",
        );
        assert!(!dest.exists());
    }
}

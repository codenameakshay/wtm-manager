//! Git mutations — the ONLY module in the crate that ever spawns a `git`
//! process. All reads go through `git2` elsewhere; mutations shell out so
//! hooks, filters, and user configuration behave exactly like the `git` CLI.
//!
//! Output policy (consistent per function):
//! - `worktree_add` / `worktree_add_new_branch` stream stdout/stderr unless
//!   quiet mode is active; quiet mode captures output and only surfaces it on
//!   failure.
//! - Everything else (`run`, `worktree_remove`, `worktree_prune`,
//!   `branch_delete`) CAPTURES output and surfaces stderr inside
//!   `Error::GitCommand` on failure.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{Error, Result};

/// Run `git <args>` with cwd = `cwd`, capturing output. Non-zero exit ⇒
/// `Error::GitCommand` with the captured stderr.
pub fn run(cwd: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::GitCommand {
            args: args.join(" "),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr)
                .trim_end()
                .to_string(),
        })
    }
}

/// Run `git <args>` with cwd = `cwd`, returning the captured output
/// regardless of exit status. Errors only if the process cannot be spawned.
pub fn run_capture(cwd: &Path, args: &[&str]) -> Result<std::process::Output> {
    Ok(Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()?)
}

/// `git worktree add <path> <branch>` (existing branch). Captures output in
/// quiet mode and streams it otherwise.
pub fn worktree_add(main_root: &Path, path: &Path, branch: &str, quiet: bool) -> Result<()> {
    let path_str = path.to_string_lossy();
    run_with_output_policy(
        main_root,
        &["worktree", "add", path_str.as_ref(), branch],
        quiet,
    )
}

/// `git worktree add -b <branch> <path> <base>` (new branch from base).
/// Captures output in quiet mode and streams it otherwise.
pub fn worktree_add_new_branch(
    main_root: &Path,
    path: &Path,
    branch: &str,
    base: &str,
    quiet: bool,
) -> Result<()> {
    let path_str = path.to_string_lossy();
    run_with_output_policy(
        main_root,
        &["worktree", "add", "-b", branch, path_str.as_ref(), base],
        quiet,
    )
}

/// `git worktree remove [--force] <path>`. Captures output.
pub fn worktree_remove(main_root: &Path, path: &Path, force: bool) -> Result<()> {
    let path_str = path.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(path_str.as_ref());
    run(main_root, &args)
}

/// `git worktree prune`. Captures output.
pub fn worktree_prune(main_root: &Path) -> Result<()> {
    run(main_root, &["worktree", "prune"])
}

/// `git branch -D <names...>` (only ever called after explicit user opt-in).
/// One spawn for any number of branches; git reports each failure and keeps
/// going, so a non-zero exit means at least one was not deleted.
pub fn branch_delete(main_root: &Path, names: &[&str]) -> Result<()> {
    let mut args = vec!["branch", "-D"];
    args.extend_from_slice(names);
    run(main_root, &args)
}

/// Run `git <args>` with cwd = `cwd`, inheriting stdout/stderr so the user
/// sees git's own output live. Non-zero exit ⇒ `Error::GitCommand`; the
/// stderr field only notes that the real output was already streamed.
fn run_streaming(cwd: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::GitCommand {
            args: args.join(" "),
            status: status.to_string(),
            stderr: "(git output was shown above)".to_string(),
        })
    }
}

fn run_with_output_policy(cwd: &Path, args: &[&str], quiet: bool) -> Result<()> {
    if quiet {
        run(cwd, args)
    } else {
        run_streaming(cwd, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testgit::{git, init_repo};
    use std::fs;
    use std::path::PathBuf;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let main = tmp.path().join("main");
        init_repo(&main);
        (tmp, main)
    }

    #[test]
    fn run_failure_carries_args_status_and_stderr() {
        let (_tmp, main) = fixture();
        let err = run(&main, &["rev-parse", "--verify", "refs/heads/nope"]).unwrap_err();
        match err {
            Error::GitCommand {
                args,
                status,
                stderr,
            } => {
                assert!(args.contains("rev-parse"), "{args}");
                assert!(!status.is_empty());
                assert!(!stderr.is_empty(), "stderr should be captured");
            }
            other => panic!("expected GitCommand, got {other}"),
        }
    }

    #[test]
    fn worktree_add_new_branch_then_remove() {
        let (tmp, main) = fixture();
        let wt = tmp.path().join("wt-feat");
        worktree_add_new_branch(&main, &wt, "feat", "main", false).unwrap();
        assert!(wt.join(".git").exists());

        worktree_remove(&main, &wt, false).unwrap();
        assert!(!wt.exists());
    }

    #[test]
    fn worktree_add_existing_branch() {
        let (tmp, main) = fixture();
        git(&main, &["branch", "other"]);
        let wt = tmp.path().join("wt-other");
        worktree_add(&main, &wt, "other", false).unwrap();
        assert!(wt.join(".git").exists());
    }

    #[test]
    fn branch_delete_removes_branch() {
        let (_tmp, main) = fixture();
        git(&main, &["branch", "dead"]);
        branch_delete(&main, &["dead"]).unwrap();
        assert!(run(&main, &["rev-parse", "--verify", "refs/heads/dead"]).is_err());
    }

    #[test]
    fn worktree_prune_cleans_stale_registry_entries() {
        let (tmp, main) = fixture();
        let wt = tmp.path().join("stale");
        worktree_add_new_branch(&main, &wt, "stale", "main", false).unwrap();
        fs::remove_dir_all(&wt).unwrap();

        worktree_prune(&main).unwrap();
        assert!(!main.join(".git").join("worktrees").join("stale").exists());
    }
}

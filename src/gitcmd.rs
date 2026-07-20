//! Git mutations — the ONLY module in the crate that ever spawns a `git`
//! process. All reads go through `git2` elsewhere; mutations shell out so
//! hooks, filters, and user configuration behave exactly like the `git` CLI.
//!
//! Output policy (consistent per function):
//! - `worktree_add` / `worktree_add_new_branch` STREAM stdout/stderr to the
//!   user (inherit) — git prints useful progress ("Preparing worktree ...").
//!   On failure the error's stderr field notes that output was already shown.
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

/// `git worktree add <path> <branch>` (existing branch). Streams output.
pub fn worktree_add(main_root: &Path, path: &Path, branch: &str) -> Result<()> {
    let path_str = path.to_string_lossy();
    run_streaming(main_root, &["worktree", "add", path_str.as_ref(), branch])
}

/// `git worktree add -b <branch> <path> <base>` (new branch from base).
/// Streams output.
pub fn worktree_add_new_branch(
    main_root: &Path,
    path: &Path,
    branch: &str,
    base: &str,
) -> Result<()> {
    let path_str = path.to_string_lossy();
    run_streaming(
        main_root,
        &["worktree", "add", "-b", branch, path_str.as_ref(), base],
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

/// `git branch -D <name>` (only ever called after explicit user opt-in).
/// Captures output.
pub fn branch_delete(main_root: &Path, name: &str) -> Result<()> {
    run(main_root, &["branch", "-D", name])
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
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

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let main = tmp.path().join("main");
        fs::create_dir(&main).unwrap();
        git(&main, &["init", "-b", "main"]);
        fs::write(main.join("README.md"), "readme\n").unwrap();
        git(&main, &["add", "."]);
        git(&main, &["commit", "-m", "initial"]);
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
        worktree_add_new_branch(&main, &wt, "feat", "main").unwrap();
        assert!(wt.join(".git").exists());

        worktree_remove(&main, &wt, false).unwrap();
        assert!(!wt.exists());
    }

    #[test]
    fn worktree_add_existing_branch() {
        let (tmp, main) = fixture();
        git(&main, &["branch", "other"]);
        let wt = tmp.path().join("wt-other");
        worktree_add(&main, &wt, "other").unwrap();
        assert!(wt.join(".git").exists());
    }

    #[test]
    fn branch_delete_removes_branch() {
        let (_tmp, main) = fixture();
        git(&main, &["branch", "dead"]);
        branch_delete(&main, "dead").unwrap();
        assert!(run(&main, &["rev-parse", "--verify", "refs/heads/dead"]).is_err());
    }

    #[test]
    fn worktree_prune_cleans_stale_registry_entries() {
        let (tmp, main) = fixture();
        let wt = tmp.path().join("stale");
        worktree_add_new_branch(&main, &wt, "stale", "main").unwrap();
        fs::remove_dir_all(&wt).unwrap();

        worktree_prune(&main).unwrap();
        assert!(!main.join(".git").join("worktrees").join("stale").exists());
    }
}

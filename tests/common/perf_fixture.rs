//! Shared fixture builder for the perf gate test and the criterion bench:
//! a repo with an initial commit and `count` linked worktrees, each on its
//! own branch.

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

pub fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {args:?} failed in {cwd:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Build a fixture repo with an initial commit and `count` linked worktrees,
/// each on its own branch. Returns the `TempDir` (kept alive for the
/// lifetime of the caller) and the main working tree's path.
pub fn build_fixture(count: usize) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("create tempdir");
    let repo_dir = tmp.path().join("repo");
    std::fs::create_dir_all(&repo_dir).expect("create repo dir");

    run_git(&repo_dir, &["init", "-b", "main"]);
    run_git(
        &repo_dir,
        &[
            "-c",
            "user.name=wtm perf",
            "-c",
            "user.email=perf@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "initial commit",
        ],
    );

    for i in 0..count {
        let branch = format!("br{i}");
        let worktree_dir = tmp.path().join(format!("wt{i}"));
        let worktree_dir_str = worktree_dir.to_str().expect("utf8 fixture path");
        run_git(
            &repo_dir,
            &["worktree", "add", "-b", &branch, worktree_dir_str, "main"],
        );
    }

    (tmp, repo_dir)
}

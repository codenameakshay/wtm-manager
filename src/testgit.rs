//! Shared git test fixtures: a hermetic `git` invocation (fixed identity, no
//! user/system config) plus helpers to bootstrap a one-commit repository.

use std::fs;
use std::path::Path;
use std::process::Command;

/// Run `git <args>` in `dir` with a fixed test identity and no user/system
/// config, so tests never depend on (or pollute) the machine's real git
/// config. Panics on failure; returns trimmed stdout for callers that need
/// it (e.g. a resolved sha).
pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Create `dir` and initialize a repo there (branch "main", one commit
/// adding README.md).
pub(crate) fn init_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-b", "main"]);
    fs::write(dir.join("README.md"), "readme\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "initial"]);
}

/// Write `file` (content: its own name) and commit it.
pub(crate) fn commit_file(dir: &Path, file: &str) {
    fs::write(dir.join(file), file).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", file]);
}

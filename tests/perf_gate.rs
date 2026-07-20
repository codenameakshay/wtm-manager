//! Non-flaky CI performance gate for `wtm list` (with status).
//!
//! Not run by default — it's `#[ignore]`d so `cargo test` stays fast and
//! deterministic. CI runs it explicitly, in release mode:
//!
//! ```sh
//! cargo test --release --test perf_gate -- --ignored --nocapture
//! ```
//!
//! The real performance budget documented in `DESIGN.md`/`README.md` is
//! ~50ms for 10-20 worktrees; this gate asserts a much more generous 250ms
//! median over 11 timed runs, to absorb CI machine variance without ever
//! being flaky.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use wtm::worktree::{self, ListOptions};

const WORKTREE_COUNT: usize = 15;
const RUNS: usize = 11;
const BUDGET: Duration = Duration::from_millis(250);

fn run_git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .expect("spawn git");
    assert!(status.success(), "git {args:?} failed in {cwd:?}");
}

/// Build a fixture repo with an initial commit and `count` linked worktrees,
/// each on its own branch. Returns the `TempDir` (kept alive for the
/// lifetime of the test) and the main working tree's path.
fn build_fixture(count: usize) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("create tempdir");
    let repo_dir = tmp.path().join("repo");
    std::fs::create_dir_all(&repo_dir).expect("create repo dir");

    run_git(&repo_dir, &["init", "-b", "main"]);
    run_git(
        &repo_dir,
        &[
            "-c",
            "user.name=wtm perf gate",
            "-c",
            "user.email=perf-gate@example.com",
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

#[test]
#[ignore]
fn list_with_status_stays_under_budget() {
    let (_tmp, repo_dir) = build_fixture(WORKTREE_COUNT);
    let ctx = wtm::repo::discover(Some(&repo_dir)).expect("discover fixture repo");
    let opts = ListOptions {
        with_status: true,
        base: None,
    };

    // Warm up once (page in the binary, prime OS file caches) before taking
    // timed measurements, so the first sample doesn't skew the median.
    worktree::list(&ctx, &opts).expect("warmup list");

    let mut samples = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let start = Instant::now();
        worktree::list(&ctx, &opts).expect("list with status");
        samples.push(start.elapsed());
    }

    samples.sort();
    let median = samples[samples.len() / 2];

    println!("list-with-status over {RUNS} runs ({WORKTREE_COUNT} worktrees): {samples:?}");
    println!("median: {median:?} (budget: {BUDGET:?})");

    assert!(
        median < BUDGET,
        "median list-with-status time {median:?} exceeded the {BUDGET:?} CI budget"
    );
}

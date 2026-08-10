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
//! This gate uses 64 linked worktrees, records both cold and warm behavior,
//! and keeps generous CI budgets so it catches regressions without measuring
//! runner noise.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use wtm::worktree::{self, ListOptions};

const WORKTREE_COUNT: usize = 64;
const RUNS: usize = 11;
const FIRST_LOAD_BUDGET: Duration = Duration::from_secs(1);
const WARM_MEDIAN_BUDGET: Duration = Duration::from_millis(500);

fn run_git(cwd: &Path, args: &[&str]) {
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

    let first_start = Instant::now();
    worktree::list(&ctx, &opts).expect("first list");
    let first_load = first_start.elapsed();

    let mut samples = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let start = Instant::now();
        worktree::list(&ctx, &opts).expect("list with status");
        samples.push(start.elapsed());
    }

    samples.sort();
    let median = samples[samples.len() / 2];

    println!(
        "list-with-status over {RUNS} warm runs ({WORKTREE_COUNT} linked worktrees): {samples:?}"
    );
    println!("first load: {first_load:?} (budget: {FIRST_LOAD_BUDGET:?})");
    println!("warm median: {median:?} (budget: {WARM_MEDIAN_BUDGET:?})");

    assert!(
        first_load < FIRST_LOAD_BUDGET,
        "first list-with-status time {first_load:?} exceeded the {FIRST_LOAD_BUDGET:?} CI budget"
    );
    assert!(
        median < WARM_MEDIAN_BUDGET,
        "warm median list-with-status time {median:?} exceeded the {WARM_MEDIAN_BUDGET:?} CI budget"
    );
}

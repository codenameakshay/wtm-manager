//! Non-flaky CI performance gate for `wtm list` (with status).
//!
//! Not run by default — it's `#[ignore]`d so `cargo test` stays fast and
//! deterministic. CI runs it explicitly, in release mode:
//!
//! ```sh
//! cargo test --release --test perf_gate -- --ignored --nocapture
//! ```
//!
//! The real-world target documented in README.md is well under ~50ms for a
//! repo with 10-20 worktrees; the budgets enforced here (first load under 1
//! second, warm median under 500ms) are deliberately looser CI-noise-tolerant
//! ceilings, not typical local timings. This gate uses 64 linked worktrees,
//! records both cold and warm behavior, and keeps generous CI budgets so it
//! catches regressions without measuring runner noise.

use std::time::{Duration, Instant};

use wtm::worktree::{self, ListOptions};

#[path = "common/perf_fixture.rs"]
mod perf_fixture;

const WORKTREE_COUNT: usize = 64;
const RUNS: usize = 11;
const FIRST_LOAD_BUDGET: Duration = Duration::from_secs(1);
const WARM_MEDIAN_BUDGET: Duration = Duration::from_millis(500);

#[test]
#[ignore]
fn list_with_status_stays_under_budget() {
    let (_tmp, repo_dir) = perf_fixture::build_fixture(WORKTREE_COUNT);
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

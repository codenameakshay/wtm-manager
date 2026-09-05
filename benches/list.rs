//! Criterion benchmarks for the internal `wtm::worktree::list` pipeline.
//!
//! `git` is only shelled out to while building the fixture repo (setup, not
//! timed); the benchmarked code path itself goes through the public
//! `wtm::repo`/`wtm::worktree` API exactly as `DESIGN.md` declares it and
//! never spawns a process.

use criterion::{criterion_group, criterion_main, Criterion};
use wtm::worktree::{self, ListOptions};

#[path = "../tests/common/perf_fixture.rs"]
mod perf_fixture;

const WORKTREE_COUNT: usize = 64;

fn bench_list(c: &mut Criterion) {
    // Setup happens once, outside the timed loop: build the fixture and
    // resolve the RepoContext a single time, then reuse it across samples.
    let (_tmp, repo_dir) = perf_fixture::build_fixture(WORKTREE_COUNT);
    let ctx = wtm::repo::discover(Some(&repo_dir)).expect("discover fixture repo");

    c.bench_function("list_with_status", |b| {
        b.iter(|| {
            worktree::list(
                &ctx,
                &ListOptions {
                    with_status: true,
                    base: None,
                },
            )
            .expect("list with status")
        })
    });

    c.bench_function("list_without_status", |b| {
        b.iter(|| {
            worktree::list(
                &ctx,
                &ListOptions {
                    with_status: false,
                    base: None,
                },
            )
            .expect("list without status")
        })
    });
}

criterion_group!(benches, bench_list);
criterion_main!(benches);

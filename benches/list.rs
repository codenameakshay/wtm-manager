//! Criterion benchmarks for the internal `wtm::worktree::list` pipeline.
//!
//! `git` is only shelled out to while building the fixture repo (setup, not
//! timed); the benchmarked code path itself goes through the public
//! `wtm::repo`/`wtm::worktree` API exactly as `DESIGN.md` declares it and
//! never spawns a process.

use std::path::{Path, PathBuf};
use std::process::Command;

use criterion::{criterion_group, criterion_main, Criterion};
use tempfile::TempDir;
use wtm::worktree::{self, ListOptions};

const WORKTREE_COUNT: usize = 15;

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
/// lifetime of the benchmark) and the main working tree's path.
fn build_fixture(count: usize) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("create tempdir");
    let repo_dir = tmp.path().join("repo");
    std::fs::create_dir_all(&repo_dir).expect("create repo dir");

    run_git(&repo_dir, &["init", "-b", "main"]);
    run_git(
        &repo_dir,
        &[
            "-c",
            "user.name=wtm bench",
            "-c",
            "user.email=bench@example.com",
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

fn bench_list(c: &mut Criterion) {
    // Setup happens once, outside the timed loop: build the fixture and
    // resolve the RepoContext a single time, then reuse it across samples.
    let (_tmp, repo_dir) = build_fixture(WORKTREE_COUNT);
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

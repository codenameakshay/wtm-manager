//! `wtm fetch`: remote selection, output format, `--quiet`, and error paths.

mod common;

use common::TestRepo;
use predicates::prelude::*;

/// Add an `origin` remote (a separate throwaway repo with one commit `repo`
/// does not have yet), so the first fetch always has something to report.
fn add_origin(repo: &TestRepo) {
    let origin = repo.base().join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    repo.git(&origin, &["init", "-b", "main"]);
    repo.commit_file_in(&origin, "seed.txt", "seed\n");
    repo.git(
        repo.root(),
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
}

#[test]
fn fetch_prints_summary_and_succeeds() {
    let repo = TestRepo::new();
    add_origin(&repo);

    repo.wtm()
        .args(["fetch"])
        .assert()
        .success()
        .stdout(predicate::str::contains("fetched origin ("));
}

#[test]
fn second_fetch_reports_no_refs_updated() {
    let repo = TestRepo::new();
    add_origin(&repo);

    repo.wtm().args(["fetch"]).assert().success();
    repo.wtm()
        .args(["fetch"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no refs updated"));
}

#[test]
fn quiet_fetch_prints_nothing() {
    let repo = TestRepo::new();
    add_origin(&repo);

    repo.wtm()
        .args(["fetch", "-q"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn fetch_unknown_remote_fails() {
    let repo = TestRepo::new();
    add_origin(&repo);

    repo.wtm()
        .args(["fetch", "--remote", "nope"])
        .assert()
        .failure();
}

#[test]
fn fetch_with_no_remotes_fails_clearly() {
    let repo = TestRepo::new();

    repo.wtm()
        .args(["fetch"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no configured remotes"));
}

#[test]
fn fetch_works_with_dash_c_from_outside_the_repo() {
    let repo = TestRepo::new();
    add_origin(&repo);
    let elsewhere = repo.base().join("unrelated");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let root_str = repo.root().to_str().unwrap().to_string();

    repo.wtm_in(&elsewhere)
        .args(["-C", &root_str, "fetch"])
        .assert()
        .success()
        .stdout(predicate::str::contains("fetched origin ("));
}

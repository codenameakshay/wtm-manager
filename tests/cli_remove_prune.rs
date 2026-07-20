//! Safety and cleanup coverage: dirty-worktree refusal, --force,
//! --with-branch, main-worktree refusal, missing-directory handling, and
//! prune (--dry-run, --merged, protected branches).

mod common;

use common::{canon, find_entry, TestRepo};
use predicates::prelude::*;

#[test]
fn remove_refuses_dirty_worktree_without_force() {
    let repo = TestRepo::new();
    repo.wtm().args(["add", "feature-x"]).assert().success();
    let wt = canon(&repo.default_worktree_path("feature-x"));

    // An untracked file makes the worktree dirty.
    std::fs::write(wt.join("untracked.txt"), "dirty\n").unwrap();

    repo.wtm()
        .args(["remove", "feature-x"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--force"));
    assert!(wt.is_dir(), "dirty worktree must survive a refused remove");

    repo.wtm()
        .args(["remove", "feature-x", "--force"])
        .assert()
        .success();
    assert!(!wt.exists(), "--force must remove the dirty worktree");
}

#[test]
fn remove_keeps_branch_unless_with_branch() {
    let repo = TestRepo::new();

    repo.wtm().args(["add", "keeper"]).assert().success();
    repo.wtm().args(["remove", "keeper"]).assert().success();
    assert!(
        repo.branch_exists("keeper"),
        "the branch must survive a plain remove"
    );

    repo.wtm().args(["add", "goner"]).assert().success();
    repo.wtm()
        .args(["remove", "goner", "--with-branch"])
        .assert()
        .success();
    assert!(
        !repo.branch_exists("goner"),
        "--with-branch must delete the branch after removal"
    );
}

#[test]
fn remove_refuses_main_worktree() {
    let repo = TestRepo::new();
    // Run from an unrelated cwd so the "you are standing in it" rule cannot
    // fire first — this isolates the main-worktree refusal.
    let elsewhere = repo.base().join("unrelated");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let root_str = repo.root().to_str().unwrap().to_string();

    repo.wtm_in(&elsewhere)
        .args(["-C", &root_str, "remove", "main"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("main worktree"));
    assert!(
        repo.root().join(".git").exists(),
        "the main worktree must be untouched"
    );
}

#[test]
fn missing_dir_listed_then_pruned() {
    let repo = TestRepo::new();
    repo.wtm().args(["add", "vanish"]).assert().success();
    let wt = canon(&repo.default_worktree_path("vanish"));

    // Simulate an rm -rf'd worktree: registry entry remains, dir is gone.
    std::fs::remove_dir_all(&wt).unwrap();

    let items = repo.list_json(&[]);
    let entry = find_entry(&items, "vanish")
        .expect("registry entry must still be listed after its directory is deleted");
    assert_eq!(entry["is_missing"], true);

    // Dry run previews the candidate without touching the registry.
    repo.wtm()
        .args(["prune", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("vanish"));
    assert!(
        repo.registry_porcelain().contains("vanish"),
        "--dry-run must not modify the registry"
    );

    // Real prune cleans the registry entry.
    repo.wtm().args(["prune"]).assert().success();
    assert!(
        !repo.registry_porcelain().contains("vanish"),
        "prune must drop the stale registry entry"
    );
}

#[test]
fn prune_merged_removes_worktree_and_branch() {
    let repo = TestRepo::new();
    repo.wtm().args(["add", "merged-feat"]).assert().success();
    let wt = canon(&repo.default_worktree_path("merged-feat"));

    // Do real work on the branch, then merge it into main.
    repo.commit_file_in(&wt, "feature.txt", "feature work\n");
    repo.git(
        repo.root(),
        &["merge", "--no-ff", "-m", "merge merged-feat", "merged-feat"],
    );

    // Dry run lists it but removes nothing.
    repo.wtm()
        .args(["prune", "--merged", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("merged-feat"));
    assert!(wt.is_dir(), "--dry-run must not remove the worktree");
    assert!(repo.branch_exists("merged-feat"));

    // Real prune removes the worktree AND deletes the merged branch.
    repo.wtm().args(["prune", "--merged"]).assert().success();
    assert!(!wt.exists(), "merged worktree must be pruned");
    assert!(
        !repo.branch_exists("merged-feat"),
        "pruning a merged worktree must delete its branch"
    );
}

#[test]
fn prune_merged_never_touches_protected_branches() {
    let repo = TestRepo::new();
    // "develop" is in the default protected_branches list.
    repo.wtm().args(["add", "develop"]).assert().success();
    let wt = canon(&repo.default_worktree_path("develop"));

    repo.commit_file_in(&wt, "dev.txt", "dev work\n");
    repo.git(
        repo.root(),
        &["merge", "--no-ff", "-m", "merge develop", "develop"],
    );

    repo.wtm().args(["prune", "--merged"]).assert().success();
    assert!(
        wt.is_dir(),
        "a protected branch's worktree must never be pruned"
    );
    assert!(
        repo.branch_exists("develop"),
        "a protected branch must never be deleted"
    );
}

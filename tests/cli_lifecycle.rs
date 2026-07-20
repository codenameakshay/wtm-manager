//! End-to-end lifecycle coverage: add → list → path → switch → remove, plus
//! creation variants (`--from`, `--path`), branch-in-use refusal,
//! registry-based detection of raw worktrees, `-C`, and invocation from
//! inside a linked worktree.

mod common;

use std::path::Path;

use common::{canon, entry_path, find_entry, stdout_str, TestRepo};
use predicates::prelude::*;

#[test]
fn full_lifecycle_add_list_path_switch_remove() {
    let repo = TestRepo::new();

    // add (non-TTY) creates the worktree at the default template location
    // ../<repo>-worktrees/feature-x with the branch checked out.
    repo.wtm().args(["add", "feature-x"]).assert().success();
    let wt = canon(&repo.default_worktree_path("feature-x"));
    assert!(
        wt.is_dir(),
        "worktree should exist at the default template location {}",
        wt.display()
    );
    assert_eq!(
        repo.git(&wt, &["branch", "--show-current"]).trim(),
        "feature-x",
        "the new branch must be checked out in the new worktree"
    );

    // list --json includes it with the right path.
    let items = repo.list_json(&[]);
    let entry = find_entry(&items, "feature-x").expect("feature-x appears in list --json");
    assert_eq!(canon(&entry_path(entry)), wt);
    assert_eq!(entry["is_main"], false);

    // path prints its path (stdout trimmed == path).
    let assert = repo.wtm().args(["path", "feature-x"]).assert().success();
    let printed = stdout_str(&assert);
    assert_eq!(canon(Path::new(printed.trim())), wt);

    // switch --print-path prints ONLY the path on stdout.
    let assert = repo
        .wtm()
        .args(["switch", "feature-x", "--print-path"])
        .assert()
        .success();
    let printed = stdout_str(&assert);
    assert_eq!(
        printed.lines().count(),
        1,
        "--print-path stdout must be exactly one line, got {printed:?}"
    );
    assert_eq!(canon(Path::new(printed.trim())), wt);

    // remove (clean worktree) succeeds and it disappears from the list.
    repo.wtm().args(["remove", "feature-x"]).assert().success();
    assert!(!wt.exists(), "removed worktree directory must be gone");
    let items = repo.list_json(&[]);
    assert!(
        find_entry(&items, "feature-x").is_none(),
        "removed worktree must not appear in list --json"
    );
}

#[test]
fn add_from_creates_branch_from_base_not_head() {
    let repo = TestRepo::new();

    // Pin a base ref, then move HEAD past it on main.
    let base_sha = repo.rev_parse(repo.root(), "HEAD");
    repo.git(repo.root(), &["branch", "stable"]);
    repo.commit_file("moved.txt", "HEAD moves on\n");
    let head_sha = repo.rev_parse(repo.root(), "HEAD");
    assert_ne!(base_sha, head_sha, "fixture: HEAD must have moved");

    repo.wtm()
        .args(["add", "feat-y", "--from", "stable"])
        .assert()
        .success();

    let wt = canon(&repo.default_worktree_path("feat-y"));
    assert_eq!(
        repo.rev_parse(&wt, "HEAD"),
        base_sha,
        "new branch must start at the --from base, not the moved HEAD"
    );
    assert_eq!(
        repo.git(&wt, &["branch", "--show-current"]).trim(),
        "feat-y"
    );
}

#[test]
fn add_path_override_places_worktree_exactly_there() {
    let repo = TestRepo::new();
    let custom = repo.base().join("custom").join("spot");

    repo.wtm()
        .args(["add", "feat-z", "--path", custom.to_str().unwrap()])
        .assert()
        .success();

    let wt = canon(&custom);
    assert!(
        wt.is_dir(),
        "worktree must be exactly at the --path override"
    );
    assert_eq!(
        repo.git(&wt, &["branch", "--show-current"]).trim(),
        "feat-z"
    );
    assert!(
        !repo.default_worktree_path("feat-z").exists(),
        "the template location must not be used when --path is given"
    );
}

#[test]
fn add_refuses_branch_checked_out_elsewhere() {
    let repo = TestRepo::new();
    let raw = repo.base().join("raw-feat-a");
    repo.add_worktree_raw("feat-a", &raw);

    repo.wtm()
        .args(["add", "feat-a"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already checked out"));
    assert!(
        !repo.default_worktree_path("feat-a").exists(),
        "no worktree may be created for a branch that is checked out elsewhere"
    );
}

#[test]
fn list_includes_raw_worktrees_at_non_template_locations() {
    let repo = TestRepo::new();
    // Deliberately deep, non-template location, created with raw git.
    let deep = repo
        .base()
        .join("elsewhere")
        .join("deep")
        .join("nested")
        .join("wt");
    repo.add_worktree_raw("offbeat", &deep);

    let items = repo.list_json(&[]);
    let entry =
        find_entry(&items, "offbeat").expect("raw worktree must be discovered via the registry");
    assert_eq!(canon(&entry_path(entry)), canon(&deep));
    assert_eq!(entry["branch"], "offbeat");
    assert_eq!(entry["is_main"], false);
}

#[test]
fn dash_c_operates_on_repo_from_unrelated_cwd() {
    let repo = TestRepo::new();
    let elsewhere = repo.base().join("unrelated");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let root_str = repo.root().to_str().unwrap().to_string();

    let assert = repo
        .wtm_in(&elsewhere)
        .args(["-C", &root_str, "list", "--json"])
        .assert()
        .success();
    let items: serde_json::Value = serde_json::from_str(&stdout_str(&assert)).unwrap();
    let main = find_entry(&items, "main").expect("main worktree visible via -C");
    assert_eq!(main["is_main"], true);

    let assert = repo
        .wtm_in(&elsewhere)
        .args(["-C", &root_str, "path"])
        .assert()
        .success();
    assert_eq!(
        canon(Path::new(stdout_str(&assert).trim())),
        canon(repo.root()),
        "-C ... path with no name must print the main root"
    );
}

#[test]
fn list_from_inside_linked_worktree_subdir_resolves_main_registry() {
    let repo = TestRepo::new();
    repo.wtm().args(["add", "feature-x"]).assert().success();
    let wt = canon(&repo.default_worktree_path("feature-x"));
    let sub = wt.join("some").join("sub");
    std::fs::create_dir_all(&sub).unwrap();

    let items = repo.list_json_in(&sub, &[]);
    let main = find_entry(&items, "main")
        .expect("main worktree must be listed when invoked from a linked worktree subdir");
    assert_eq!(main["is_main"], true);
    assert_eq!(canon(&entry_path(main)), canon(repo.root()));
    assert!(
        find_entry(&items, "feature-x").is_some(),
        "the linked worktree itself must be listed too"
    );
}

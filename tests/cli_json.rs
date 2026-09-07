//! `--json` output shape and the status-skipping flags (`--no-status`,
//! alias `--fast`).

mod common;

use common::{find_entry, TestRepo};

/// Stable field names declared in src/model.rs — the JSON contract.
const EXPECTED_FIELDS: &[&str] = &[
    "name",
    "path",
    "branch",
    "head",
    "is_main",
    "is_missing",
    "is_locked",
    "lock_reason",
    "head_time",
    "is_prunable",
    "status",
];

const STATUS_FIELDS: &[&str] = &[
    "dirty",
    "dirty_count",
    "ahead",
    "behind",
    "upstream_gone",
    "merged",
];

#[test]
fn json_has_stable_shape_with_status_by_default() {
    let repo = TestRepo::new();
    repo.wtm().args(["add", "feature-x"]).assert().success();

    let items = repo.list_json(&[]);
    let arr = items.as_array().expect("list --json is an array");
    assert!(
        arr.len() >= 2,
        "main + linked worktree expected, got {items}"
    );

    for entry in arr {
        let obj = entry.as_object().expect("entry is an object");
        for field in EXPECTED_FIELDS {
            assert!(obj.contains_key(*field), "entry missing `{field}`: {entry}");
        }
    }

    let main = find_entry(&items, "main").expect("main entry present");
    assert_eq!(main["is_main"], true);
    assert_eq!(main["name"], "main", "main worktree uses the literal name");
    assert_eq!(main["is_missing"], false);

    // Status is computed by default.
    let status = &main["status"];
    let status_obj = status
        .as_object()
        .unwrap_or_else(|| panic!("status must be computed by default, got {status}"));
    for field in STATUS_FIELDS {
        assert!(
            status_obj.contains_key(*field),
            "status missing `{field}`: {status}"
        );
    }
    // Clean fresh repo without a remote: not dirty, no upstream.
    assert_eq!(status["dirty"], false);
    assert_eq!(status["dirty_count"], 0);
    assert!(status["ahead"].is_null(), "no upstream => ahead is null");
    assert!(status["behind"].is_null(), "no upstream => behind is null");
}

#[test]
fn json_no_status_and_fast_alias_yield_null_status() {
    let repo = TestRepo::new();
    repo.wtm().args(["add", "feature-x"]).assert().success();

    for flag in ["--no-status", "--fast"] {
        let items = repo.list_json(&[flag]);
        for entry in items.as_array().expect("array") {
            assert!(
                entry["status"].is_null(),
                "{flag}: status must be null, got {entry}"
            );
        }
    }
}

#[test]
fn json_exposes_lock_reason_when_git_locks_a_worktree() {
    let repo = TestRepo::new();
    repo.wtm().args(["add", "locked"]).assert().success();
    let wt = repo.default_worktree_path("locked");
    let path = wt.to_str().expect("utf-8 path");
    repo.git(
        repo.root(),
        &["worktree", "lock", path, "--reason", "agent-in-use"],
    );

    let items = repo.list_json(&["--fast"]);
    let locked = find_entry(&items, "locked").expect("locked worktree");
    assert_eq!(locked["is_locked"], true);
    assert_eq!(locked["lock_reason"], "agent-in-use");

    let main = find_entry(&items, "main").expect("main");
    assert_eq!(main["is_locked"], false);
    assert!(main["lock_reason"].is_null());
}

#[test]
fn json_fast_still_exposes_head_time() {
    let repo = TestRepo::new();
    repo.wtm().args(["add", "feature-x"]).assert().success();

    let items = repo.list_json(&["--fast"]);
    for entry in items.as_array().expect("array") {
        assert!(
            entry["status"].is_null(),
            "--fast: status must be null, got {entry}"
        );
        assert!(
            entry["head_time"].is_number(),
            "--fast: head_time must be a unix timestamp, got {entry}"
        );
    }
}

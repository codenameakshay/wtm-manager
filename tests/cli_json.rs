//! `--json` output shape and the status-skipping flags (`--no-status`,
//! alias `--fast`).

mod common;

use common::{find_entry, TestRepo};
use predicates::prelude::*;

/// Stable field names declared in src/model.rs — the JSON contract.
const EXPECTED_FIELDS: &[&str] = &[
    "name",
    "path",
    "branch",
    "head",
    "is_main",
    "is_missing",
    "is_locked",
    "is_prunable",
    "status",
];

const STATUS_FIELDS: &[&str] = &["dirty", "ahead", "behind", "upstream_gone", "merged"];

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
fn list_table_accepts_no_status_and_fast_alias() {
    let repo = TestRepo::new();
    repo.wtm()
        .args(["list", "--no-status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("main"));
    repo.wtm()
        .args(["list", "--fast"])
        .assert()
        .success()
        .stdout(predicate::str::contains("main"));
}

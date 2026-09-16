//! `wtm host` — saved remote hosts, and scan/rm/prune over a fake `ssh` that
//! runs the piped script locally, so the "remote" host is the local machine.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{stdout_str, TestRepo};
use predicates::prelude::*;

fn host_add(
    repo: &TestRepo,
    ssh: &Path,
    destination: &str,
    roots: &[&Path],
) -> assert_cmd::assert::Assert {
    let mut cmd = repo.wtm();
    cmd.env("WTM_SSH", ssh);
    cmd.args(["host", "add", "vps", destination]);
    for root in roots {
        cmd.arg("--root").arg(root);
    }
    cmd.assert()
}

fn host_list_json(repo: &TestRepo, ssh: &Path) -> serde_json::Value {
    let assert = repo
        .wtm()
        .env("WTM_SSH", ssh)
        .args(["host", "list", "--json"])
        .assert()
        .success();
    serde_json::from_str(&stdout_str(&assert)).expect("valid JSON")
}

#[test]
fn host_add_list_forget_round_trip() {
    let repo = TestRepo::new();
    let ssh = common::write_fake_ssh(repo.base());

    let base = repo.base().to_path_buf();
    repo.wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "add", "vps", "ubuntu@example.com"])
        .arg("--root")
        .arg(base.join("code"))
        .arg("--root")
        .arg(base.join("work"))
        .assert()
        .success();

    let list = host_list_json(&repo, &ssh);
    let arr = list.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "vps");
    assert_eq!(arr[0]["destination"], "ubuntu@example.com");
    assert_eq!(
        arr[0]["roots"],
        serde_json::json!([
            base.join("code").to_string_lossy(),
            base.join("work").to_string_lossy(),
        ])
    );

    host_add(&repo, &ssh, "ubuntu@other.example.com", &[]).success();
    let list = host_list_json(&repo, &ssh);
    let arr = list.as_array().expect("array");
    assert_eq!(arr.len(), 1, "re-adding must replace, not duplicate");
    assert_eq!(arr[0]["destination"], "ubuntu@other.example.com");

    repo.wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "forget", "vps"])
        .assert()
        .success();
    assert_eq!(host_list_json(&repo, &ssh), serde_json::json!([]));

    repo.wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "forget", "vps"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no host named 'vps'"));
}

#[test]
fn host_add_rejects_bad_destination() {
    let repo = TestRepo::new();
    let ssh = common::write_fake_ssh(repo.base());

    repo.wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "add", "vps", "--", "-oProxyCommand=evil"])
        .assert()
        .failure();

    assert_eq!(host_list_json(&repo, &ssh), serde_json::json!([]));
}

#[test]
fn host_scan_json_lists_repo_worktrees_and_sizes() {
    let repo = TestRepo::new();
    let ssh = common::write_fake_ssh(repo.base());
    repo.wtm().args(["add", "feature-a"]).assert().success();
    repo.wtm().args(["add", "feature-b"]).assert().success();

    host_add(&repo, &ssh, "ubuntu@example.com", &[repo.base()]).success();

    let assert = repo
        .wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "scan", "vps", "--json"])
        .assert()
        .success();
    let repos: serde_json::Value = serde_json::from_str(&stdout_str(&assert)).expect("valid JSON");
    let arr = repos.as_array().expect("array of repos");
    assert_eq!(arr.len(), 1);
    let repo_obj = &arr[0];
    assert_eq!(repo_obj["name"], "repo");
    assert_eq!(
        PathBuf::from(repo_obj["path"].as_str().expect("path")),
        repo.root()
    );

    let worktrees = repo_obj["worktrees"].as_array().expect("worktrees array");
    assert_eq!(worktrees.len(), 3);
    assert_eq!(worktrees[0]["is_main"], true, "main worktree listed first");

    let mut branches: Vec<String> = worktrees
        .iter()
        .map(|w| w["branch"].as_str().expect("branch").to_string())
        .collect();
    branches.sort();
    assert_eq!(branches, ["feature-a", "feature-b", "main"]);

    for w in worktrees {
        if w["is_missing"] == serde_json::json!(false) {
            let size = w["size_bytes"].as_u64().expect("numeric size_bytes");
            assert!(size > 0, "expected a positive size, got {w}");
        }
    }
}

#[test]
fn host_scan_no_size_omits_sizes_and_text_table_shows_repo() {
    let repo = TestRepo::new();
    let ssh = common::write_fake_ssh(repo.base());
    repo.wtm().args(["add", "feature-a"]).assert().success();

    host_add(&repo, &ssh, "ubuntu@example.com", &[repo.base()]).success();

    let assert = repo
        .wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "scan", "vps", "--no-size", "--json"])
        .assert()
        .success();
    let repos: serde_json::Value = serde_json::from_str(&stdout_str(&assert)).expect("valid JSON");
    for repo_obj in repos.as_array().expect("array") {
        for w in repo_obj["worktrees"].as_array().expect("worktrees") {
            assert!(w["size_bytes"].is_null(), "expected null size, got {w}");
        }
    }

    let assert = repo
        .wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "scan", "vps"])
        .assert()
        .success();
    let stdout = stdout_str(&assert);
    assert!(stdout.contains("repo"), "{stdout}");
    assert!(stdout.contains("feature-a"), "{stdout}");
    assert!(stdout.contains("main"), "{stdout}");
}

#[test]
fn host_rm_removes_worktree_and_branch() {
    let repo = TestRepo::new();
    let ssh = common::write_fake_ssh(repo.base());
    repo.wtm().args(["add", "feature-a"]).assert().success();
    let path = repo.default_worktree_path("feature-a");

    host_add(&repo, &ssh, "ubuntu@example.com", &[repo.base()]).success();

    let assert = repo
        .wtm()
        .env("WTM_SSH", &ssh)
        .arg("host")
        .arg("rm")
        .arg("vps")
        .arg(&path)
        .arg("--with-branch")
        .arg("--json")
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_str(&stdout_str(&assert)).expect("valid JSON");
    assert_eq!(v["ok"], true);
    assert_eq!(v["branch_deleted"], true);

    assert!(!path.exists());
    assert!(repo
        .git(repo.root(), &["branch", "--list", "feature-a"])
        .trim()
        .is_empty());
}

#[test]
fn host_rm_refuses_dirty_without_force() {
    let repo = TestRepo::new();
    let ssh = common::write_fake_ssh(repo.base());
    repo.wtm().args(["add", "feature-a"]).assert().success();
    let path = repo.default_worktree_path("feature-a");
    fs::write(path.join("scratch.txt"), "x").expect("write scratch file");

    host_add(&repo, &ssh, "ubuntu@example.com", &[repo.base()]).success();

    repo.wtm()
        .env("WTM_SSH", &ssh)
        .arg("host")
        .arg("rm")
        .arg("vps")
        .arg(&path)
        .assert()
        .failure();
    assert!(path.exists());

    repo.wtm()
        .env("WTM_SSH", &ssh)
        .arg("host")
        .arg("rm")
        .arg("vps")
        .arg(&path)
        .arg("--force")
        .assert()
        .success();
    assert!(!path.exists());
}

#[test]
fn host_rm_refuses_main_worktree() {
    let repo = TestRepo::new();
    let ssh = common::write_fake_ssh(repo.base());
    host_add(&repo, &ssh, "ubuntu@example.com", &[repo.base()]).success();

    repo.wtm()
        .env("WTM_SSH", &ssh)
        .arg("host")
        .arg("rm")
        .arg("vps")
        .arg(repo.root())
        .assert()
        .failure()
        .stderr(predicate::str::contains("main worktree"));
}

#[test]
fn host_rm_with_branch_refuses_protected_branch() {
    let repo = TestRepo::new();
    let ssh = common::write_fake_ssh(repo.base());
    host_add(&repo, &ssh, "ubuntu@example.com", &[repo.base()]).success();
    repo.wtm().args(["add", "develop"]).assert().success();
    let path = repo.default_worktree_path("develop");

    repo.wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "rm", "vps"])
        .arg(&path)
        .arg("--with-branch")
        .assert()
        .failure()
        .stderr(predicate::str::contains("branch 'develop' is protected"));
    assert!(path.exists());
}

#[test]
fn host_rm_with_branch_respects_repo_protected_branches() {
    let repo = TestRepo::new();
    let ssh = common::write_fake_ssh(repo.base());
    repo.wtm().args(["add", "keep"]).assert().success();
    let keep_path = repo.default_worktree_path("keep");

    repo.write_repo_config("[prune]\nprotected_branches = [\"main\", \"keep\"]\n");

    host_add(&repo, &ssh, "ubuntu@example.com", &[repo.base()]).success();

    repo.wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "rm", "vps"])
        .arg(&keep_path)
        .arg("--with-branch")
        .assert()
        .failure()
        .stderr(predicate::str::contains("branch 'keep' is protected"));
    assert!(keep_path.exists());
}

#[test]
fn host_scan_fails_on_invalid_repo_config() {
    let repo = TestRepo::new();
    let ssh = common::write_fake_ssh(repo.base());
    repo.write_repo_config("not toml [[[");

    host_add(&repo, &ssh, "ubuntu@example.com", &[repo.base()]).success();

    repo.wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "scan", "vps"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(".worktree.toml"));
}

#[test]
fn host_add_warns_about_locally_expanded_root() {
    let repo = TestRepo::new();
    let ssh = common::write_fake_ssh(repo.base());
    let root = repo.base().join("home").join("projects");

    repo.wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "add", "vps", "x"])
        .arg("--root")
        .arg(&root)
        .assert()
        .success()
        .stderr(predicate::str::contains("quote it: --root '~/projects'"));
}

#[test]
fn host_prune_merged_dry_run_then_prune() {
    let repo = TestRepo::new();
    let ssh = common::write_fake_ssh(repo.base());
    repo.wtm().args(["add", "feature-a"]).assert().success();
    repo.wtm().args(["add", "feature-b"]).assert().success();
    let path_a = repo.default_worktree_path("feature-a");
    let path_b = repo.default_worktree_path("feature-b");
    repo.commit_file_in(&path_b, "b.txt", "b");

    host_add(&repo, &ssh, "ubuntu@example.com", &[repo.base()]).success();

    let assert = repo
        .wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "prune", "vps", "--merged", "--dry-run", "--json"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_str(&stdout_str(&assert)).expect("valid JSON");
    let candidates: Vec<String> = v["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .map(|c| c.as_str().expect("string").to_string())
        .collect();
    assert_eq!(candidates, ["feature-a"]);
    assert_eq!(v["removed"], 0);
    assert!(v["reclaimable_bytes"].as_u64().expect("numeric") > 0, "{v}");
    assert!(path_a.exists());

    let assert = repo
        .wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "prune", "vps", "--merged", "--json"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_str(&stdout_str(&assert)).expect("valid JSON");
    assert_eq!(v["removed"], 1, "{v}");
    assert!(!path_a.exists());
    assert!(repo
        .git(repo.root(), &["branch", "--list", "feature-a"])
        .trim()
        .is_empty());
    assert!(path_b.exists());
}

#[test]
fn host_prune_respects_repo_protected_branches() {
    let repo = TestRepo::new();
    let ssh = common::write_fake_ssh(repo.base());
    repo.wtm().args(["add", "keep"]).assert().success();
    repo.wtm().args(["add", "drop"]).assert().success();
    let keep_path = repo.default_worktree_path("keep");
    let drop_path = repo.default_worktree_path("drop");

    repo.write_repo_config("[prune]\nprotected_branches = [\"main\", \"keep\"]\n");

    host_add(&repo, &ssh, "ubuntu@example.com", &[repo.base()]).success();

    let assert = repo
        .wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "prune", "vps", "--merged", "--json"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_str(&stdout_str(&assert)).expect("valid JSON");
    let candidates: Vec<String> = v["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .map(|c| c.as_str().expect("string").to_string())
        .collect();
    assert_eq!(candidates, ["drop"]);
    assert_eq!(v["removed"], 1, "{v}");
    assert!(keep_path.exists());
    assert!(!drop_path.exists());
}

#[test]
fn host_unknown_name_errors() {
    let repo = TestRepo::new();
    let ssh = common::write_fake_ssh(repo.base());

    repo.wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "scan", "nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no host named 'nope'"));
}

#[test]
fn host_ssh_connection_failure_is_reported_without_login_hint() {
    let repo = TestRepo::new();
    let ssh = repo.base().join("fake-ssh-fail");
    common::write_executable_script(
        &ssh,
        "#!/bin/sh\necho 'ssh: connect to host x port 22: Connection refused' >&2\nexit 255\n",
    );

    repo.wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "add", "vps", "ubuntu@example.com"])
        .assert()
        .success();

    repo.wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "scan", "vps"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Connection refused"))
        .stderr(predicate::str::contains("BatchMode=yes").not());
}

#[test]
fn host_ssh_login_failure_suggests_an_interactive_login() {
    let repo = TestRepo::new();
    let ssh = repo.base().join("fake-ssh-denied");
    common::write_executable_script(
        &ssh,
        "#!/bin/sh\necho 'ubuntu@example.com: Permission denied (publickey).' >&2\nexit 255\n",
    );
    repo.wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "add", "vps", "ubuntu@example.com"])
        .assert()
        .success();

    repo.wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "scan", "vps"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Permission denied (publickey)."))
        .stderr(predicate::str::contains(
            "run `ssh ubuntu@example.com` in a terminal once",
        ));
}

#[test]
fn host_ssh_ignores_configured_port_forwards() {
    let repo = TestRepo::new();
    let args_file = repo.base().join("ssh-args");
    let ssh = repo.base().join("fake-ssh-forward");
    common::write_executable_script(
        &ssh,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncase \" $* \" in\n  *' ClearAllForwardings=yes '*) exec sh -s ;;\nesac\necho 'Error: remote port forwarding failed for listen port 8317' >&2\nexit 255\n",
            args_file.display()
        ),
    );
    host_add(&repo, &ssh, "ubuntu@example.com", &[repo.base()]).success();

    repo.wtm()
        .env("WTM_SSH", &ssh)
        .args(["host", "scan", "vps", "--no-size", "--json"])
        .assert()
        .success();

    let args = std::fs::read_to_string(&args_file).unwrap();
    let args: Vec<&str> = args.lines().collect();
    assert!(args.contains(&"ClearAllForwardings=yes"), "{args:?}");
    assert!(args.contains(&"-a") && args.contains(&"-x"), "{args:?}");
    let dash_dash = args.iter().position(|a| *a == "--").unwrap();
    assert_eq!(&args[dash_dash + 1..], ["ubuntu@example.com", "sh -s"]);
}

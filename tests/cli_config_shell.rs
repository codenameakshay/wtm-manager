//! Config layering (.worktree.toml: path_template, setup.copy,
//! setup.commands, --no-setup), `config init`/`config path`, shell
//! integration (`init`, `completions`), and non-TTY / error UX.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{canon, stdout_str, TestRepo};
use predicates::prelude::*;

// ---------------------------------------------------------------------------
// Non-TTY and error UX
// ---------------------------------------------------------------------------

#[test]
fn remove_without_name_non_tty_fails_fast_instead_of_hanging() {
    let repo = TestRepo::new();
    repo.wtm()
        .arg("remove")
        .write_stdin("")
        .timeout(Duration::from_secs(10))
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires a name argument"));
}

#[test]
fn path_without_args_prints_main_root() {
    let repo = TestRepo::new();
    let assert = repo.wtm().arg("path").assert().success();
    assert_eq!(
        canon(Path::new(stdout_str(&assert).trim())),
        canon(repo.root()),
        "`wtm path` inside the main repo must print the main root"
    );
}

#[test]
fn path_unknown_name_fails_helpfully() {
    let repo = TestRepo::new();
    repo.wtm()
        .args(["path", "nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nope"));
}

// ---------------------------------------------------------------------------
// Shell integration
// ---------------------------------------------------------------------------

#[test]
fn init_emits_shell_wrapper_for_zsh_and_bash() {
    let repo = TestRepo::new();
    for shell in ["zsh", "bash"] {
        repo.wtm().args(["init", shell]).assert().success().stdout(
            predicate::str::contains("command wtm switch --print-path")
                .and(predicate::str::contains("wtm()")),
        );
    }
}

#[test]
fn completions_emit_shell_scripts() {
    let repo = TestRepo::new();
    repo.wtm()
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_wtm"));
    repo.wtm()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("complete").and(predicate::str::contains("wtm")));
}

// ---------------------------------------------------------------------------
// Repo config: path_template
// ---------------------------------------------------------------------------

#[test]
fn repo_config_relative_path_template_lands_under_main_root() {
    let repo = TestRepo::new();
    repo.write_repo_config("path_template = \"wts/{slug}\"\n");

    repo.wtm()
        .args(["add", "feat/slug-test"])
        .assert()
        .success();

    // Relative template => under the main root; {slug} turns '/' into '-'.
    let wt = canon(&repo.root().join("wts").join("feat-slug-test"));
    assert!(wt.is_dir(), "worktree must land at wts/feat-slug-test");
    assert_eq!(
        repo.git(&wt, &["branch", "--show-current"]).trim(),
        "feat/slug-test",
        "the branch keeps its raw (slashed) name"
    );
}

// ---------------------------------------------------------------------------
// Repo config: setup.copy + setup.commands
// ---------------------------------------------------------------------------

const SETUP_CONFIG: &str = r#"
[setup]
commands = ["touch setup-ran"]
copy = [{ path = ".env" }]
"#;

#[test]
fn setup_copies_files_and_runs_commands_in_new_worktree() {
    let repo = TestRepo::new();
    // Untracked file in the main worktree, copied by setup.copy.
    std::fs::write(repo.root().join(".env"), "SECRET=1\n").unwrap();
    repo.write_repo_config(SETUP_CONFIG);

    repo.wtm().args(["add", "with-setup"]).assert().success();

    let wt = canon(&repo.default_worktree_path("with-setup"));
    assert_eq!(
        std::fs::read_to_string(wt.join(".env")).expect(".env must be copied into the worktree"),
        "SECRET=1\n"
    );
    assert!(
        wt.join("setup-ran").is_file(),
        "setup commands must run with cwd = the new worktree"
    );
}

#[test]
fn no_setup_skips_copy_and_commands() {
    let repo = TestRepo::new();
    std::fs::write(repo.root().join(".env"), "SECRET=1\n").unwrap();
    repo.write_repo_config(SETUP_CONFIG);

    repo.wtm()
        .args(["add", "without-setup", "--no-setup"])
        .assert()
        .success();

    let wt = canon(&repo.default_worktree_path("without-setup"));
    assert!(wt.is_dir());
    assert!(
        !wt.join(".env").exists(),
        "--no-setup must skip setup.copy entries"
    );
    assert!(
        !wt.join("setup-ran").exists(),
        "--no-setup must skip setup.commands"
    );
}

// ---------------------------------------------------------------------------
// wtm config subcommands
// ---------------------------------------------------------------------------

#[test]
fn config_init_scaffolds_and_refuses_rerun() {
    let repo = TestRepo::new();

    repo.wtm().args(["config", "init"]).assert().success();
    assert!(
        repo.root().join(".worktree.toml").is_file(),
        "config init must scaffold .worktree.toml at the repo root"
    );

    repo.wtm()
        .args(["config", "init"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("exist"));
}

#[test]
fn config_path_prints_config_locations() {
    let repo = TestRepo::new();
    repo.wtm()
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains("config.toml"));
}

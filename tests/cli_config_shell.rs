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
fn init_emits_cd_file_shell_wrapper_for_zsh_and_bash() {
    let repo = TestRepo::new();
    for shell in ["zsh", "bash"] {
        repo.wtm().args(["init", shell]).assert().success().stdout(
            predicate::str::contains("WTM_CD_FILE=\"$cdfile\" command wtm")
                .and(predicate::str::contains("builtin cd -- \"$target\"")),
        );
    }
}

// ---------------------------------------------------------------------------
// cd-on-exit (WTM_CD_FILE) mechanism
// ---------------------------------------------------------------------------

#[test]
fn switch_writes_cd_file_when_wrapper_is_active() {
    let repo = TestRepo::new();
    repo.wtm().args(["add", "feat"]).assert().success();

    let cd_file = repo.base().join("wtm-cd.switch");
    std::fs::write(&cd_file, "").unwrap();
    let assert = repo
        .wtm()
        .env("WTM_CD_FILE", &cd_file)
        .args(["switch", "feat"])
        .assert()
        .success();

    let recorded = std::fs::read_to_string(&cd_file).expect("switch must write the cd file");
    let recorded = recorded.strip_suffix('.').expect("cd sentinel");
    assert_eq!(
        canon(Path::new(recorded)),
        canon(&repo.default_worktree_path("feat")),
        "cd file must hold the target worktree path"
    );
    assert!(
        stdout_str(&assert).trim().is_empty(),
        "with the wrapper active, stdout stays empty (the cd file carries the path)"
    );
}

#[test]
fn switch_print_path_prints_even_with_cd_file() {
    let repo = TestRepo::new();
    repo.wtm().args(["add", "feat"]).assert().success();

    let cd_file = repo.base().join("wtm-cd.print");
    std::fs::write(&cd_file, "").unwrap();
    let assert = repo
        .wtm()
        .env("WTM_CD_FILE", &cd_file)
        .args(["switch", "feat", "--print-path"])
        .assert()
        .success();

    assert_eq!(
        canon(Path::new(stdout_str(&assert).trim())),
        canon(&repo.default_worktree_path("feat")),
        "--print-path must keep printing the path for scripts"
    );
    assert!(cd_file.is_file(), "the cd file is still written");
}

#[test]
fn switch_without_wrapper_prints_path_and_init_hint() {
    let repo = TestRepo::new();
    repo.wtm().args(["add", "feat"]).assert().success();

    let assert = repo
        .wtm()
        .args(["switch", "feat"])
        .assert()
        .success()
        .stderr(predicate::str::contains("wtm init"));
    assert_eq!(
        canon(Path::new(stdout_str(&assert).trim())),
        canon(&repo.default_worktree_path("feat"))
    );
}

#[test]
fn add_cd_writes_cd_file_when_wrapper_is_active() {
    let repo = TestRepo::new();
    let cd_file = repo.base().join("wtm-cd.add");
    std::fs::write(&cd_file, "").unwrap();
    repo.wtm()
        .env("WTM_CD_FILE", &cd_file)
        .args(["add", "feat", "--cd"])
        .assert()
        .success();

    let recorded = std::fs::read_to_string(&cd_file).expect("add --cd must write the cd file");
    let recorded = recorded.strip_suffix('.').expect("cd sentinel");
    assert_eq!(
        canon(Path::new(recorded)),
        canon(&repo.default_worktree_path("feat"))
    );
}

#[test]
fn add_cd_does_not_request_switch_when_setup_fails() {
    let repo = TestRepo::new();
    std::fs::write(
        repo.root().join(".worktree.local.toml"),
        "[setup]\ncommands = [\"exit 7\"]\n",
    )
    .unwrap();
    let cd_file = repo.base().join("wtm-cd.failed-setup");
    std::fs::write(&cd_file, "").unwrap();

    repo.wtm()
        .env("WTM_CD_FILE", &cd_file)
        .args(["add", "failed-setup", "--cd"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("setup step failed"));

    assert!(
        std::fs::read(&cd_file).unwrap().is_empty(),
        "failed setup must leave the shell handoff empty"
    );
    assert!(repo.default_worktree_path("failed-setup").is_dir());
}

#[test]
fn quiet_add_and_remove_emit_no_success_or_git_progress() {
    let repo = TestRepo::new();
    repo.wtm()
        .args(["--quiet", "add", "silent"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::is_empty());
    repo.wtm()
        .args(["--quiet", "remove", "silent"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::is_empty());
}

#[test]
fn invalid_editor_is_reported_before_success() {
    let repo = TestRepo::new();
    std::fs::write(
        repo.root().join(".worktree.local.toml"),
        "editor = \"definitely-not-a-real-wtm-editor\"\n",
    )
    .unwrap();
    repo.wtm()
        .args(["open", "main"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(
            predicate::str::contains("editor command is not available")
                .and(predicate::str::contains("opened").not()),
        );
}

#[cfg(unix)]
#[test]
fn quoted_editor_path_with_spaces_passes_preflight() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TestRepo::new();
    let editor_dir = repo.base().join("editor tools");
    std::fs::create_dir(&editor_dir).unwrap();
    let editor = editor_dir.join("mock editor");
    std::fs::write(&editor, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
    let command = format!("'{}' --reuse-window", editor.display());
    std::fs::write(
        repo.root().join(".worktree.local.toml"),
        format!("editor = {:?}\n", command),
    )
    .unwrap();

    repo.wtm()
        .args(["open", "main"])
        .assert()
        .success()
        .stderr(predicate::str::contains("opened"));
}

// ---------------------------------------------------------------------------
// Bare `wtm` in non-TTY contexts
// ---------------------------------------------------------------------------

#[test]
fn bare_wtm_non_tty_prints_help_and_exits_zero() {
    let repo = TestRepo::new();
    repo.wtm()
        .write_stdin("")
        .timeout(Duration::from_secs(10))
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage").and(predicate::str::contains("wtm")));
}

#[test]
fn completions_emit_shell_scripts() {
    let repo = TestRepo::new();
    repo.wtm()
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_wtm"));
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
copy = [{ path = ".env" }]
"#;

const LOCAL_SETUP_CONFIG: &str = r#"
[setup]
commands = ["touch setup-ran"]
"#;

#[test]
fn setup_copies_files_and_runs_commands_in_new_worktree() {
    let repo = TestRepo::new();
    // Untracked file in the main worktree, copied by setup.copy.
    std::fs::write(repo.root().join(".env"), "SECRET=1\n").unwrap();
    repo.write_repo_config(SETUP_CONFIG);
    std::fs::write(repo.root().join(".worktree.local.toml"), LOCAL_SETUP_CONFIG).unwrap();

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
    std::fs::write(repo.root().join(".worktree.local.toml"), LOCAL_SETUP_CONFIG).unwrap();

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

#[test]
fn shared_repo_setup_commands_are_rejected_with_actionable_error() {
    let repo = TestRepo::new();
    repo.write_repo_config(
        r#"
[setup]
commands = ["touch should-not-run"]
"#,
    );

    repo.wtm()
        .args(["add", "blocked"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains(".worktree.toml")
                .and(predicate::str::contains("setup.commands"))
                .and(predicate::str::contains(".worktree.local.toml")),
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

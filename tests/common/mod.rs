//! Shared fixtures for `wtm` integration tests.
//!
//! Everything here is hermetic: no user git config, no user wtm config, no
//! network. Each [`TestRepo`] owns its own temp directories, so tests are
//! parallel-safe. The test process's own cwd is never changed — child
//! commands always get an explicit `.current_dir(...)`.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::time::Duration;

use assert_cmd::Command;
use tempfile::TempDir;

/// Environment variables scrubbed from every child process so nothing from
/// the developer's real session (git identity, config, editors, wtm config)
/// can leak into a test.
const SCRUBBED_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
    "GIT_EDITOR",
    "GIT_PAGER",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_ASKPASS",
    "XDG_CONFIG_HOME",
    "HOME",
    "VISUAL",
    "EDITOR",
    "WTM_CONFIG_DIR",
];

/// Hermetic environment values applied on top of the scrub list.
fn hermetic_env_pairs(home: &Path, wtm_config: &Path) -> Vec<(&'static str, String)> {
    vec![
        ("HOME", home.display().to_string()),
        (
            "XDG_CONFIG_HOME",
            home.join(".config").display().to_string(),
        ),
        ("GIT_CONFIG_GLOBAL", "/dev/null".to_string()),
        ("GIT_CONFIG_SYSTEM", "/dev/null".to_string()),
        ("GIT_CONFIG_NOSYSTEM", "1".to_string()),
        ("GIT_TERMINAL_PROMPT", "0".to_string()),
        ("WTM_CONFIG_DIR", wtm_config.display().to_string()),
        ("NO_COLOR", "1".to_string()),
    ]
}

/// A hermetic `wtm` command bound to the given HOME and wtm config dir.
pub fn wtm_with(home: &Path, wtm_config: &Path) -> Command {
    let mut cmd = Command::cargo_bin("wtm").expect("wtm binary should build");
    cmd.timeout(Duration::from_secs(60));
    for key in SCRUBBED_ENV {
        cmd.env_remove(key);
    }
    for (key, value) in hermetic_env_pairs(home, wtm_config) {
        cmd.env(key, value);
    }
    cmd
}

/// A hermetic `wtm` command with a throwaway HOME and an empty wtm config
/// dir. The temp dirs are intentionally kept alive until process exit (via
/// `mem::forget`) so the command can outlive this function. Prefer
/// [`TestRepo::wtm`] whenever a fixture repo exists.
pub fn wtm() -> Command {
    let home = TempDir::new().expect("create hermetic HOME");
    let config = TempDir::new().expect("create hermetic WTM_CONFIG_DIR");
    fs::create_dir_all(home.path().join(".config")).expect("create .config in hermetic HOME");
    let cmd = wtm_with(home.path(), config.path());
    std::mem::forget(home);
    std::mem::forget(config);
    cmd
}

/// A throwaway real git repository plus the hermetic HOME / wtm-config dirs
/// every command run against it should use.
///
/// Layout inside the owned tempdir:
/// - `<base>/home`        — hermetic $HOME (with `.config/`)
/// - `<base>/wtm-config`  — empty $WTM_CONFIG_DIR
/// - `<base>/<name>`      — the main worktree (default name: `repo`), so the
///   default template `../{repo}-worktrees/{branch}` stays inside the tempdir.
pub struct TestRepo {
    tmp: TempDir,
    base: PathBuf,
    home: PathBuf,
    wtm_config: PathBuf,
    root: PathBuf,
}

impl TestRepo {
    /// New repo named `repo` on branch `main` with one seed commit.
    pub fn new() -> Self {
        Self::with_repo_name("repo")
    }

    pub fn with_repo_name(name: &str) -> Self {
        let tmp = TempDir::new().expect("create tempdir");
        // Canonicalize up front so every derived path compares equal on
        // macOS (/tmp vs /private/tmp).
        let base = tmp.path().canonicalize().expect("canonicalize tempdir");
        let home = base.join("home");
        let wtm_config = base.join("wtm-config");
        let root = base.join(name);
        fs::create_dir_all(home.join(".config")).expect("create hermetic HOME");
        fs::create_dir_all(&wtm_config).expect("create hermetic WTM_CONFIG_DIR");
        fs::create_dir_all(&root).expect("create repo dir");

        let repo = TestRepo {
            tmp,
            base,
            home,
            wtm_config,
            root,
        };
        repo.git(repo.root(), &["init", "-b", "main"]);
        repo.commit_file("README.md", "seed\n");
        repo
    }

    /// Absolute, canonicalized path of the main worktree root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Canonicalized tempdir base — scratch space *outside* the repo, useful
    /// for custom worktree destinations and unrelated cwds.
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Where the default template `../{repo}-worktrees/{branch}` puts a
    /// worktree for `name`.
    pub fn default_worktree_path(&self, name: &str) -> PathBuf {
        let repo_name = self
            .root
            .file_name()
            .expect("repo root has a name")
            .to_string_lossy();
        self.root
            .parent()
            .expect("repo root has a parent")
            .join(format!("{repo_name}-worktrees"))
            .join(name)
    }

    /// A hermetic `wtm` command with cwd = the main worktree root.
    pub fn wtm(&self) -> Command {
        self.wtm_in(&self.root)
    }

    /// A hermetic `wtm` command with an explicit cwd.
    pub fn wtm_in(&self, dir: &Path) -> Command {
        let mut cmd = wtm_with(&self.home, &self.wtm_config);
        cmd.current_dir(dir);
        cmd
    }

    /// Run a fixture git command hermetically (fixed identity, no signing),
    /// panicking on failure. Returns captured stdout.
    pub fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let mut cmd = StdCommand::new("git");
        cmd.current_dir(cwd);
        for key in SCRUBBED_ENV {
            cmd.env_remove(key);
        }
        for (key, value) in hermetic_env_pairs(&self.home, &self.wtm_config) {
            cmd.env(key, value);
        }
        cmd.args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
        ]);
        cmd.args(args);
        let out = cmd.output().expect("spawn git");
        assert!(
            out.status.success(),
            "fixture `git {}` in {} failed ({}):\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            cwd.display(),
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Write + `git add` + `git commit` a file in the main worktree.
    pub fn commit_file(&self, name: &str, content: &str) {
        let root = self.root.clone();
        self.commit_file_in(&root, name, content);
    }

    /// Write + `git add` + `git commit` a file in an arbitrary worktree.
    pub fn commit_file_in(&self, worktree: &Path, name: &str, content: &str) {
        let dest = worktree.join(name);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(&dest, content).expect("write fixture file");
        self.git(worktree, &["add", name]);
        self.git(worktree, &["commit", "-m", &format!("add {name}")]);
    }

    /// Raw `git worktree add -b <branch> <path>` — deliberately NOT via wtm,
    /// to exercise registry-based detection.
    pub fn add_worktree_raw(&self, branch: &str, path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs for raw worktree");
        }
        let path_str = path.to_str().expect("utf-8 path").to_string();
        self.git(self.root(), &["worktree", "add", "-b", branch, &path_str]);
    }

    /// `git rev-parse <spec>` in `cwd`, trimmed.
    pub fn rev_parse(&self, cwd: &Path, spec: &str) -> String {
        self.git(cwd, &["rev-parse", spec]).trim().to_string()
    }

    /// Whether a local branch exists (via `git branch --list`).
    pub fn branch_exists(&self, name: &str) -> bool {
        !self
            .git(self.root(), &["branch", "--list", name])
            .trim()
            .is_empty()
    }

    /// Raw `git worktree list --porcelain` output for registry assertions.
    pub fn registry_porcelain(&self) -> String {
        self.git(self.root(), &["worktree", "list", "--porcelain"])
    }

    /// Run `wtm list --json [extra]` from the repo root and parse the output.
    pub fn list_json(&self, extra: &[&str]) -> serde_json::Value {
        let root = self.root.clone();
        self.list_json_in(&root, extra)
    }

    /// Run `wtm list --json [extra]` from an arbitrary cwd and parse it.
    pub fn list_json_in(&self, dir: &Path, extra: &[&str]) -> serde_json::Value {
        let assert = self
            .wtm_in(dir)
            .args(["list", "--json"])
            .args(extra)
            .assert()
            .success();
        let stdout = stdout_str(&assert);
        serde_json::from_str(&stdout).unwrap_or_else(|err| {
            panic!("`wtm list --json` did not emit valid JSON: {err}\n---\n{stdout}")
        })
    }

    /// Write `<repo>/.worktree.toml`.
    pub fn write_repo_config(&self, contents: &str) {
        fs::write(self.root.join(".worktree.toml"), contents).expect("write .worktree.toml");
    }
}

/// Canonicalize, panicking with the offending path on failure (macOS
/// /tmp-vs-/private/tmp means both sides of a comparison must go through
/// this).
pub fn canon(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|err| panic!("canonicalize {}: {err}", path.display()))
}

/// Find the `list --json` entry whose branch OR registry name equals `name`.
pub fn find_entry<'a>(items: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    items
        .as_array()
        .expect("list --json output is an array")
        .iter()
        .find(|entry| {
            entry["branch"].as_str() == Some(name) || entry["name"].as_str() == Some(name)
        })
}

/// The `path` field of a `list --json` entry.
pub fn entry_path(entry: &serde_json::Value) -> PathBuf {
    PathBuf::from(entry["path"].as_str().expect("entry.path is a string"))
}

/// Captured stdout of a finished assert as UTF-8.
pub fn stdout_str(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).expect("stdout is UTF-8")
}

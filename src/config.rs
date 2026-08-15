//! Layered configuration: built-in defaults < global config < repo config <
//! repo-local config, merged field-by-field so a layer that only sets one
//! key never clobbers keys set by an earlier layer.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const REPO_CONFIG_FILENAME: &str = ".worktree.toml";
const LOCAL_CONFIG_FILENAME: &str = ".worktree.local.toml";

/// On-disk shape of a single config layer (global, repo, or repo-local
/// TOML file). Every field is optional: absence means "inherit from an
/// earlier layer", not "reset to the built-in default".
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub path_template: Option<String>,
    pub default_base: Option<String>,
    pub editor: Option<String>,
    pub setup: Option<SetupConfigFile>,
    pub prune: Option<PruneConfigFile>,
}

/// On-disk shape of the `[setup]` table.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SetupConfigFile {
    pub commands: Option<Vec<String>>,
    pub copy: Option<Vec<CopyEntry>>,
}

/// On-disk shape of the `[prune]` table.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PruneConfigFile {
    pub protected_branches: Option<Vec<String>>,
}

/// Fully resolved configuration after merging every layer.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Path template for new worktrees. Default `"../{repo}-worktrees/{branch}"`.
    pub path_template: String,
    /// Base ref for new branches / merged detection. `None` means "use HEAD".
    pub default_base: Option<String>,
    /// Editor override. Resolution order at use site: config > $VISUAL > $EDITOR.
    pub editor: Option<String>,
    pub setup: SetupConfig,
    pub prune: PruneConfig,
}

/// Post-create automation configuration.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SetupConfig {
    pub commands: Vec<String>,
    pub copy: Vec<CopyEntry>,
}

/// A single file/directory to copy or symlink into every new worktree.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct CopyEntry {
    pub path: String,
    #[serde(default)]
    pub mode: CopyMode,
}

/// How a [`CopyEntry`] is materialized in the new worktree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CopyMode {
    #[default]
    Copy,
    Symlink,
}

/// `wtm prune` safety configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct PruneConfig {
    pub protected_branches: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            path_template: "../{repo}-worktrees/{branch}".to_string(),
            default_base: None,
            editor: None,
            setup: SetupConfig::default(),
            prune: PruneConfig::default(),
        }
    }
}

impl Default for PruneConfig {
    fn default() -> Self {
        PruneConfig {
            protected_branches: vec![
                "main".to_string(),
                "master".to_string(),
                "develop".to_string(),
            ],
        }
    }
}

/// Layered load: built-in defaults < global config
/// (`$WTM_CONFIG_DIR`/config.toml, falling back to
/// `$XDG_CONFIG_HOME/wtm/config.toml`, falling back to
/// `~/.config/wtm/config.toml`) < `<repo_root>/.worktree.toml` <
/// `<repo_root>/.worktree.local.toml`. Later layers override earlier ones
/// field-by-field; missing files are silently skipped. An unparseable file
/// produces `Error::Config` naming the file path.
pub fn load(repo_root: &Path) -> Result<Config> {
    let mut cfg = Config::default();

    if let Some(global_path) = global_config_path() {
        if let Some(layer) = load_layer(&global_path)? {
            cfg = merge(cfg, layer);
        }
    }

    if let Some(layer) = load_layer(&repo_root.join(REPO_CONFIG_FILENAME))? {
        if layer.editor.is_some() {
            return Err(Error::Config(format!(
                "{REPO_CONFIG_FILENAME}: editor cannot be loaded from shared repository config because it is executed as a command; move editor to {LOCAL_CONFIG_FILENAME} or the global config"
            )));
        }
        if layer
            .setup
            .as_ref()
            .and_then(|setup| setup.commands.as_ref())
            .is_some()
        {
            return Err(Error::Config(format!(
                "{REPO_CONFIG_FILENAME}: setup.commands cannot be loaded from shared repository config; move commands to {LOCAL_CONFIG_FILENAME} or the global config"
            )));
        }
        cfg = merge(cfg, layer);
    }

    if let Some(layer) = load_layer(&repo_root.join(LOCAL_CONFIG_FILENAME))? {
        cfg = merge(cfg, layer);
    }

    Ok(cfg)
}

/// Merge a parsed file layer over `base`, field-by-field. `setup.commands`,
/// `setup.copy`, and `prune.protected_branches` REPLACE the base value
/// entirely when present in `layer` (they never append/merge as lists).
pub fn merge(base: Config, layer: ConfigFile) -> Config {
    let mut cfg = base;

    if let Some(path_template) = layer.path_template {
        cfg.path_template = path_template;
    }
    if let Some(default_base) = layer.default_base {
        cfg.default_base = Some(default_base);
    }
    if let Some(editor) = layer.editor {
        cfg.editor = Some(editor);
    }
    if let Some(setup) = layer.setup {
        if let Some(commands) = setup.commands {
            cfg.setup.commands = commands;
        }
        if let Some(copy) = setup.copy {
            cfg.setup.copy = copy;
        }
    }
    if let Some(prune) = layer.prune {
        if let Some(protected_branches) = prune.protected_branches {
            cfg.prune.protected_branches = protected_branches;
        }
    }

    cfg
}

/// Directory holding wtm's user-level state, honoring the `$WTM_CONFIG_DIR`
/// override, falling back to `$XDG_CONFIG_HOME/wtm`, falling back to
/// `~/.config/wtm`. `None` only when no override is set and the home
/// directory cannot be resolved.
///
/// `config.toml` lives here, and so does the GUI's repository registry
/// (see [`crate::registry`]).
pub fn global_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("WTM_CONFIG_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("wtm"));
        }
    }
    let base_dirs = directories::BaseDirs::new()?;
    Some(base_dirs.home_dir().join(".config").join("wtm"))
}

/// Path of the global config file, honoring the `$WTM_CONFIG_DIR` override
/// (`$WTM_CONFIG_DIR/config.toml`), falling back to
/// `$XDG_CONFIG_HOME/wtm/config.toml`, falling back to
/// `~/.config/wtm/config.toml`. `None` only when no override is set and the
/// home directory cannot be resolved.
pub fn global_config_path() -> Option<PathBuf> {
    Some(global_config_dir()?.join("config.toml"))
}

/// Write a fully commented sample `.worktree.toml` at `repo_root`. Errors if
/// the file already exists. Returns the path written.
pub fn scaffold_repo_config(repo_root: &Path) -> Result<PathBuf> {
    let path = repo_root.join(REPO_CONFIG_FILENAME);
    if path.exists() {
        return Err(Error::Config(format!("{} already exists", path.display())));
    }
    std::fs::write(&path, SAMPLE_CONFIG)?;
    Ok(path)
}

/// Read and parse a single layer file. `Ok(None)` when the file does not
/// exist; `Error::Config` (naming the path) when it exists but fails to
/// parse.
fn load_layer(path: &Path) -> Result<Option<ConfigFile>> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            let parsed: ConfigFile = toml::from_str(&contents)
                .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
            Ok(Some(parsed))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Io(e)),
    }
}

const SAMPLE_CONFIG: &str = r#"# wtm repository configuration.
#
# Uncomment and edit the settings you want to override. Any key left out
# falls back to the global config (~/.config/wtm/config.toml) or wtm's
# built-in defaults. Keep `.worktree.toml` in version control for
# team-shared settings; use `.worktree.local.toml` (gitignored) for
# machine-local overrides — it takes precedence over this file.

# Template used to compute the filesystem path of a NEW worktree.
# Placeholders: {repo} {branch} {slug} {home} {repo_dir}
# path_template = "../{repo}-worktrees/{branch}"

# Base ref used when creating new branches and when computing "merged"
# status, e.g. "origin/main". Falls back to HEAD when unset.
# default_base = "origin/main"

# Executable values are intentionally excluded from shared repository config.
# Put `editor` and [setup].commands in `.worktree.local.toml` or the global
# config instead.

# Files/directories copied (or symlinked) from the main worktree into every
# new worktree, e.g. untracked local env files. Existing destination files
# are never overwritten.
# [[setup.copy]]
# path = ".env"
# mode = "copy" # or "symlink"; default "copy"

# [prune]
# Branches `wtm prune` will never touch, even with --merged/--gone.
# protected_branches = ["main", "master", "develop"]
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::sync::Mutex;

    /// Serializes tests that mutate `WTM_CONFIG_DIR` so they don't race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// RAII guard: sets `WTM_CONFIG_DIR` under the lock, restores on drop.
    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(dir: &Path) -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("WTM_CONFIG_DIR", dir);
            EnvGuard { _lock: lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("WTM_CONFIG_DIR");
        }
    }

    struct ScopedEnvVar {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl ScopedEnvVar {
        fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            ScopedEnvVar { key, previous }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn default_config_matches_spec() {
        let cfg = Config::default();
        assert_eq!(cfg.path_template, "../{repo}-worktrees/{branch}");
        assert_eq!(cfg.default_base, None);
        assert_eq!(cfg.editor, None);
        assert_eq!(cfg.setup, SetupConfig::default());
        assert_eq!(
            cfg.prune.protected_branches,
            vec![
                "main".to_string(),
                "master".to_string(),
                "develop".to_string()
            ]
        );
    }

    #[test]
    fn setup_config_default_is_empty() {
        let setup = SetupConfig::default();
        assert!(setup.commands.is_empty());
        assert!(setup.copy.is_empty());
    }

    #[test]
    fn copy_mode_defaults_to_copy() {
        assert_eq!(CopyMode::default(), CopyMode::Copy);
    }

    #[test]
    fn merge_overrides_only_set_fields() {
        let base = Config::default();
        let layer = ConfigFile {
            editor: Some("cursor".to_string()),
            ..Default::default()
        };
        let merged = merge(base.clone(), layer);
        assert_eq!(merged.editor, Some("cursor".to_string()));
        // Untouched fields survive from the base layer untouched.
        assert_eq!(merged.path_template, base.path_template);
        assert_eq!(merged.default_base, base.default_base);
        assert_eq!(merged.prune, base.prune);
    }

    #[test]
    fn merge_replaces_lists_rather_than_appending() {
        let mut base = Config::default();
        base.setup.commands = vec!["old".to_string()];
        base.prune.protected_branches = vec!["main".to_string()];

        let layer = ConfigFile {
            setup: Some(SetupConfigFile {
                commands: Some(vec!["new1".to_string(), "new2".to_string()]),
                copy: None,
            }),
            prune: Some(PruneConfigFile {
                protected_branches: Some(vec!["trunk".to_string()]),
            }),
            ..Default::default()
        };
        let merged = merge(base, layer);
        assert_eq!(
            merged.setup.commands,
            vec!["new1".to_string(), "new2".to_string()]
        );
        assert_eq!(merged.prune.protected_branches, vec!["trunk".to_string()]);
    }

    #[test]
    fn merge_is_layered_across_multiple_files() {
        // Layer 1 sets path_template only.
        let layer1 = ConfigFile {
            path_template: Some("{repo_dir}/../wt/{slug}".to_string()),
            ..Default::default()
        };
        // Layer 2 sets editor only; must not clobber layer 1's path_template.
        let layer2 = ConfigFile {
            editor: Some("vim".to_string()),
            ..Default::default()
        };
        let cfg = merge(merge(Config::default(), layer1), layer2);
        assert_eq!(cfg.path_template, "{repo_dir}/../wt/{slug}");
        assert_eq!(cfg.editor, Some("vim".to_string()));
    }

    #[test]
    fn parses_sample_toml_shape() {
        let toml_str = r#"
path_template = "../{repo}-worktrees/{branch}"
default_base  = "origin/main"
editor        = "cursor"
[setup]
commands = ["mise install", "npm install"]
[[setup.copy]]
path = ".env"
mode = "copy"
[prune]
protected_branches = ["main", "master", "develop"]
"#;
        let parsed: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            parsed.path_template,
            Some("../{repo}-worktrees/{branch}".to_string())
        );
        assert_eq!(parsed.default_base, Some("origin/main".to_string()));
        assert_eq!(parsed.editor, Some("cursor".to_string()));
        let setup = parsed.setup.unwrap();
        assert_eq!(
            setup.commands,
            Some(vec!["mise install".to_string(), "npm install".to_string()])
        );
        let copy = setup.copy.unwrap();
        assert_eq!(copy.len(), 1);
        assert_eq!(copy[0].path, ".env");
        assert_eq!(copy[0].mode, CopyMode::Copy);
        let prune = parsed.prune.unwrap();
        assert_eq!(
            prune.protected_branches,
            Some(vec![
                "main".to_string(),
                "master".to_string(),
                "develop".to_string()
            ])
        );
    }

    #[test]
    fn unparseable_toml_is_config_error_with_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(REPO_CONFIG_FILENAME);
        std::fs::write(&path, "this is not valid toml [[[").unwrap();
        let err = load_layer(&path).unwrap_err();
        match err {
            Error::Config(msg) => assert!(
                msg.contains(&path.display().to_string()),
                "message should mention path, got: {msg}"
            ),
            other => panic!("expected Error::Config, got {other:?}"),
        }
    }

    #[test]
    fn missing_layer_file_is_fine() {
        let tmp = tempfile::tempdir().unwrap();
        let result = load_layer(&tmp.path().join("does-not-exist.toml")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn load_layers_repo_then_local_over_defaults() {
        let _guard = EnvGuard::set(tempfile::tempdir().unwrap().path());
        // Empty global dir: no config.toml present, so global layer is a no-op.

        let repo_root = tempfile::tempdir().unwrap();
        std::fs::write(
            repo_root.path().join(REPO_CONFIG_FILENAME),
            "path_template = \"repo-template/{branch}\"\n",
        )
        .unwrap();
        std::fs::write(
            repo_root.path().join(LOCAL_CONFIG_FILENAME),
            "editor = \"vim\"\n",
        )
        .unwrap();

        let cfg = load(repo_root.path()).unwrap();
        // Local layer overrides editor...
        assert_eq!(cfg.editor, Some("vim".to_string()));
        // ...but doesn't clobber path_template set by the repo layer.
        assert_eq!(cfg.path_template, "repo-template/{branch}");
    }

    #[test]
    fn global_config_path_honors_wtm_config_dir_override() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set(tmp.path());
        assert_eq!(
            global_config_path().unwrap(),
            tmp.path().join("config.toml")
        );
    }

    #[test]
    fn load_reads_global_config_layer() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("config.toml"), "editor = \"nano\"\n").unwrap();
        let _guard = EnvGuard::set(tmp.path());

        let repo_root = tempfile::tempdir().unwrap();
        let cfg = load(repo_root.path()).unwrap();
        assert_eq!(cfg.editor, Some("nano".to_string()));
        assert_eq!(cfg.path_template, Config::default().path_template);
    }

    #[test]
    fn load_rejects_shared_repo_setup_commands_with_actionable_error() {
        let _guard = EnvGuard::set(tempfile::tempdir().unwrap().path());
        let repo_root = tempfile::tempdir().unwrap();
        std::fs::write(
            repo_root.path().join(REPO_CONFIG_FILENAME),
            "[setup]\ncommands = [\"touch shared\"]\n",
        )
        .unwrap();

        let err = load(repo_root.path()).unwrap_err();
        match err {
            Error::Config(message) => {
                assert!(message.contains(REPO_CONFIG_FILENAME), "{message}");
                assert!(message.contains("setup.commands"), "{message}");
                assert!(message.contains(LOCAL_CONFIG_FILENAME), "{message}");
            }
            other => panic!("expected config error, got {other:?}"),
        }
    }

    #[test]
    fn load_rejects_shared_repo_editor_with_actionable_error() {
        let global = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set(global.path());
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(REPO_CONFIG_FILENAME),
            "editor = \"code --wait\"\n",
        )
        .unwrap();

        let error = load(tmp.path()).unwrap_err();
        match error {
            Error::Config(message) => {
                assert!(message.contains("editor"), "{message}");
                assert!(message.contains(LOCAL_CONFIG_FILENAME), "{message}");
            }
            other => panic!("expected config error, got {other:?}"),
        }
    }

    #[test]
    fn load_accepts_setup_commands_from_global_and_repo_local_layers() {
        let global = tempfile::tempdir().unwrap();
        std::fs::write(
            global.path().join("config.toml"),
            "[setup]\ncommands = [\"from-global\"]\n",
        )
        .unwrap();
        let _guard = EnvGuard::set(global.path());

        let repo_root = tempfile::tempdir().unwrap();
        std::fs::write(
            repo_root.path().join(LOCAL_CONFIG_FILENAME),
            "[setup]\ncommands = [\"from-local\"]\n",
        )
        .unwrap();

        let cfg = load(repo_root.path()).unwrap();
        assert_eq!(cfg.setup.commands, vec!["from-local".to_string()]);
    }

    #[test]
    fn empty_wtm_config_dir_falls_back_to_xdg_config_home() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let xdg = tempfile::tempdir().unwrap();
        let _wtm = ScopedEnvVar::set("WTM_CONFIG_DIR", "");
        let _xdg = ScopedEnvVar::set("XDG_CONFIG_HOME", xdg.path());

        assert_eq!(
            global_config_path().unwrap(),
            xdg.path().join("wtm").join("config.toml")
        );
    }

    #[test]
    fn scaffold_repo_config_writes_parseable_file() {
        let repo_root = tempfile::tempdir().unwrap();
        let path = scaffold_repo_config(repo_root.path()).unwrap();
        assert_eq!(path, repo_root.path().join(REPO_CONFIG_FILENAME));

        let contents = std::fs::read_to_string(&path).unwrap();
        let parsed: ConfigFile = toml::from_str(&contents).unwrap();
        // Everything is commented out, so every field should be absent.
        assert_eq!(parsed, ConfigFile::default());
    }

    #[test]
    fn scaffold_repo_config_errors_if_exists() {
        let repo_root = tempfile::tempdir().unwrap();
        scaffold_repo_config(repo_root.path()).unwrap();
        let err = scaffold_repo_config(repo_root.path()).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }
}

//! GUI-local preferences: appearance, window placement, and the last
//! repository opened — everything that belongs to *this app*, not to the
//! worktree behavior `wtm` enforces from the command line.
//!
//! Deliberately separate from `wtm::config`: the CLI's TOML config
//! (`.worktree.toml`, `~/.config/wtm/config.toml`) is shared with the CLI and
//! may be checked into a repository, so the GUI must never rewrite it. This
//! module keeps its own file, `gui.json`, in the same directory
//! ([`wtm::config::global_config_dir`]) — same neighborhood, disjoint
//! ownership.
//!
//! The persistence pattern mirrors [`wtm::registry`] exactly: an atomic write
//! (temp file in the same directory, then rename) and a schema version that
//! degrades a missing, unreadable, corrupt, or newer-than-known file to
//! defaults rather than erroring. A broken preferences file is a papercut,
//! never a reason the app fails to open.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use wtm::config;

/// Current on-disk schema version. Bumped only for incompatible changes; a
/// file written by a newer version of the app is ignored rather than
/// partially trusted, so an older build can never misinterpret a shape it
/// doesn't understand.
const SCHEMA_VERSION: u32 = 1;

const PREFS_FILENAME: &str = "gui.json";

/// System appearance follow mode, independent of the OS's live light/dark
/// switch that [`crate::theme::refresh`] already reacts to — this is the
/// user's override of that default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

/// The window's last frame in screen coordinates, so the app reopens where
/// it was left instead of re-centering every launch.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowFrame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The GUI's persisted preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    /// system | light | dark
    pub appearance: Appearance,
    /// Terminal app to open worktrees in (macOS app name), `None` = Terminal.
    pub terminal: Option<String>,
    pub sidebar_visible: bool,
    pub detail_panel_visible: bool,
    /// Last window frame, so the app reopens where it was left.
    pub window: Option<WindowFrame>,
    /// Path of the repository that was open last.
    pub last_repo: Option<PathBuf>,
    /// `motion::reduced`'s persisted backing (SPEC §5's reduced-motion
    /// pref) — mirrors `motion.rs`'s own global at startup and on every
    /// toggle; see `WtmApp::set_reduce_motion`. `#[serde(default)]` so a
    /// `gui.json` written before this field existed still loads: `appearance`
    /// (this struct's oldest field) predates that attribute existing at all
    /// in this file and has no such guard of its own, so a file missing
    /// *that* key fails to parse and `load` falls back to full defaults —
    /// this field deliberately does not repeat that gap, which is the
    /// literal "older files without the key still load" case this field's
    /// tests below cover.
    #[serde(default)]
    pub reduce_motion: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            appearance: Appearance::System,
            terminal: None,
            sidebar_visible: true,
            detail_panel_visible: true,
            window: None,
            last_repo: None,
            reduce_motion: false,
        }
    }
}

/// The on-disk envelope: a schema version alongside the preferences
/// themselves, kept separate from [`Prefs`] so the version never leaks into
/// the app's in-memory API (nothing outside `load`/`save` needs to know it
/// exists).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PrefsFile {
    #[serde(default)]
    version: u32,
    #[serde(flatten)]
    prefs: Prefs,
}

/// Path of the preferences file, or `None` when no config directory can be
/// resolved (mirrors [`wtm::registry::registry_path`]).
fn prefs_path() -> Option<PathBuf> {
    Some(config::global_config_dir()?.join(PREFS_FILENAME))
}

/// Load preferences, degrading to [`Prefs::default`] when the file is
/// absent, unreadable, corrupt, or written by a schema version newer than
/// this build understands. This never fails: a broken preferences file must
/// not stop the app from opening.
pub fn load() -> Prefs {
    let Some(path) = prefs_path() else {
        return Prefs::default();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Prefs::default();
    };
    match serde_json::from_str::<PrefsFile>(&raw) {
        Ok(file) if file.version <= SCHEMA_VERSION => file.prefs,
        _ => Prefs::default(),
    }
}

/// Persist preferences, creating the config directory when needed. The write
/// goes to a temp file in the same directory and is renamed into place, so a
/// crash mid-write can never truncate the existing file.
pub fn save(prefs: &Prefs) -> Result<(), String> {
    let Some(path) = prefs_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }

    let file = PrefsFile {
        version: SCHEMA_VERSION,
        prefs: prefs.clone(),
    };
    let json = serde_json::to_string_pretty(&file)
        .map_err(|e| format!("could not serialize preferences: {e}"))?;

    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json.as_bytes())
        .map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("could not save {}: {e}", path.display()))?;
    Ok(())
}

/// Serializes tests (in this module and elsewhere in the crate — see
/// `crate::app::integration_tests`) that mutate `WTM_CONFIG_DIR`. Env vars
/// are process-global, so two tests racing on the same var under `cargo
/// test`'s default parallelism would flake without this — the same pattern
/// `wtm::config`'s own tests use for the same reason. Declared here rather
/// than nested in `mod tests` below so the integration test module (a
/// sibling of this one, not a descendant) can share the exact same lock
/// instead of racing an unrelated one of its own.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    /// RAII guard: sets `WTM_CONFIG_DIR` under the lock, restores on drop.
    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(dir: &std::path::Path) -> Self {
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

    #[test]
    fn defaults_are_sensible() {
        let prefs = Prefs::default();
        assert_eq!(prefs.appearance, Appearance::System);
        assert_eq!(prefs.terminal, None);
        assert!(prefs.sidebar_visible);
        assert!(prefs.detail_panel_visible);
        assert_eq!(prefs.window, None);
        assert_eq!(prefs.last_repo, None);
        assert!(!prefs.reduce_motion);
    }

    #[test]
    fn defaults_round_trip_through_serialize_deserialize() {
        let prefs = Prefs::default();
        let file = PrefsFile {
            version: SCHEMA_VERSION,
            prefs: prefs.clone(),
        };
        let json = serde_json::to_string(&file).unwrap();
        let parsed: PrefsFile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, SCHEMA_VERSION);
        assert_eq!(parsed.prefs, prefs);
    }

    #[test]
    fn non_default_values_round_trip_too() {
        let prefs = Prefs {
            appearance: Appearance::Dark,
            terminal: Some("iTerm".to_string()),
            sidebar_visible: false,
            detail_panel_visible: false,
            window: Some(WindowFrame {
                x: 10.0,
                y: 20.0,
                width: 1200.0,
                height: 800.0,
            }),
            last_repo: Some(PathBuf::from("/tmp/some-repo")),
            reduce_motion: true,
        };
        let file = PrefsFile {
            version: SCHEMA_VERSION,
            prefs: prefs.clone(),
        };
        let json = serde_json::to_string(&file).unwrap();
        let parsed: PrefsFile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.prefs, prefs);
    }

    #[test]
    fn a_corrupt_file_fails_to_parse_and_default_is_still_sane() {
        // The parse path is what `load` exercises against a real file on
        // disk; check it directly against a corrupt string, the same way
        // `wtm::registry`'s own corruption test does.
        let parsed = serde_json::from_str::<PrefsFile>("{ not json");
        assert!(parsed.is_err());
        assert_eq!(Prefs::default(), Prefs::default());
    }

    #[test]
    fn load_returns_defaults_when_the_file_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set(tmp.path());
        assert_eq!(load(), Prefs::default());
    }

    #[test]
    fn load_returns_defaults_when_the_file_is_corrupt() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set(tmp.path());
        std::fs::write(tmp.path().join(PREFS_FILENAME), "{ not json").unwrap();
        assert_eq!(load(), Prefs::default());
    }

    #[test]
    fn load_ignores_a_future_schema_version() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set(tmp.path());
        std::fs::write(
            tmp.path().join(PREFS_FILENAME),
            r#"{"version":999,"appearance":"dark","terminal":null,"sidebar_visible":false,"detail_panel_visible":false,"window":null,"last_repo":null,"reduce_motion":true}"#,
        )
        .unwrap();

        // A file from a future wtm-gui is ignored wholesale rather than
        // partially trusted, matching wtm::registry's rule.
        assert_eq!(load(), Prefs::default());
    }

    #[test]
    fn load_of_an_older_file_without_reduce_motion_still_loads_the_rest() {
        // A `gui.json` written before `reduce_motion` existed has no such
        // key at all. `#[serde(default)]` on the field is what keeps this
        // loading (falling back to `false` for just that field) rather than
        // failing to parse the whole file the way a missing `appearance`
        // key would (see the field's own doc) — every other saved value
        // must survive intact.
        let tmp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set(tmp.path());
        std::fs::write(
            tmp.path().join(PREFS_FILENAME),
            r#"{"version":1,"appearance":"dark","terminal":"iTerm","sidebar_visible":false,"detail_panel_visible":false,"window":null,"last_repo":null}"#,
        )
        .unwrap();

        let loaded = load();
        assert!(!loaded.reduce_motion, "missing key defaults to false");
        assert_eq!(loaded.appearance, Appearance::Dark);
        assert_eq!(loaded.terminal, Some("iTerm".to_string()));
        assert!(!loaded.sidebar_visible);
        assert!(!loaded.detail_panel_visible);
    }

    #[test]
    fn save_then_load_round_trips_through_the_real_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set(tmp.path());

        let prefs = Prefs {
            appearance: Appearance::Light,
            terminal: Some("Alacritty".to_string()),
            sidebar_visible: false,
            detail_panel_visible: true,
            window: Some(WindowFrame {
                x: 0.0,
                y: 0.0,
                width: 1024.0,
                height: 768.0,
            }),
            last_repo: Some(tmp.path().join("repo")),
            reduce_motion: true,
        };

        save(&prefs).unwrap();
        assert_eq!(load(), prefs);
        assert!(tmp.path().join(PREFS_FILENAME).exists());
        // The atomic-write temp file never survives a successful save.
        assert!(!tmp.path().join("gui.json.tmp").exists());
    }

    #[test]
    fn save_creates_the_config_directory_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("nested").join("config");
        let _guard = EnvGuard::set(&nested);

        save(&Prefs::default()).unwrap();
        assert!(nested.join(PREFS_FILENAME).exists());
    }
}

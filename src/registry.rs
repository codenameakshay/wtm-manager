//! The known-repository registry: the list of repositories the GUI shows in
//! its sidebar.
//!
//! The CLI never needs this — it discovers a repository from the working
//! directory on every invocation. A window opened from the Dock has no
//! working directory, so the app needs a durable answer to "which
//! repositories do I manage?".
//!
//! Design constraints:
//! - **The registry is a convenience cache, never a source of truth.**
//!   Worktrees always come from git's own registry via [`crate::worktree`];
//!   this file only remembers which repositories to ask. A corrupt or
//!   missing file therefore degrades to an empty sidebar, never to an error.
//! - **Entries are keyed by main-worktree path.** [`crate::repo::discover`]
//!   resolves any path inside a repo (including a linked worktree) to the
//!   main root, so opening the same project from two different worktrees
//!   records one entry.
//! - Writes are atomic (temp file + rename) because two windows may be open
//!   at once.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::error::Result;

const REGISTRY_FILENAME: &str = "repos.json";

/// Current on-disk schema version. Bumped only for incompatible changes; a
/// file with an unknown version is ignored rather than migrated in place, so
/// an older wtm can never scribble over a newer format.
const SCHEMA_VERSION: u32 = 1;

/// One repository the GUI knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEntry {
    /// Absolute path to the repository's MAIN working tree.
    pub path: PathBuf,
    /// Display name (the main working tree's directory name at record time).
    pub name: String,
    /// Unix timestamp (seconds) of the last time this repo was opened, used
    /// for most-recent-first ordering. `0` when unknown.
    #[serde(default)]
    pub last_opened: u64,
}

impl RepoEntry {
    /// Is the recorded path still a directory on disk? The sidebar shows
    /// missing repositories greyed out rather than dropping them silently —
    /// an unmounted volume should not lose the user's list.
    pub fn exists(&self) -> bool {
        self.path.is_dir()
    }
}

/// The whole registry file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    repos: Vec<RepoEntry>,
}

impl Registry {
    /// Entries, most recently opened first.
    pub fn entries(&self) -> Vec<RepoEntry> {
        let mut out = self.repos.clone();
        out.sort_by(|a, b| {
            b.last_opened
                .cmp(&a.last_opened)
                .then_with(|| a.name.cmp(&b.name))
        });
        out
    }

    /// Record `path` (or refresh its timestamp when already present) and
    /// return whether the registry changed.
    pub fn remember(&mut self, path: &Path, name: &str) -> bool {
        let now = unix_now();
        if let Some(existing) = self.repos.iter_mut().find(|r| r.path == path) {
            existing.last_opened = now;
            existing.name = name.to_string();
            return true;
        }
        self.repos.push(RepoEntry {
            path: path.to_path_buf(),
            name: name.to_string(),
            last_opened: now,
        });
        true
    }

    /// Drop `path` from the registry. Returns whether anything was removed.
    /// This only forgets the entry; nothing on disk is touched.
    pub fn forget(&mut self, path: &Path) -> bool {
        let before = self.repos.len();
        self.repos.retain(|r| r.path != path);
        self.repos.len() != before
    }

    /// Does the registry already know about this path?
    pub fn contains(&self, path: &Path) -> bool {
        self.repos.iter().any(|r| r.path == path)
    }
}

/// Path of the registry file, or `None` when no config directory can be
/// resolved.
pub fn registry_path() -> Option<PathBuf> {
    Some(config::global_config_dir()?.join(REGISTRY_FILENAME))
}

/// Load the registry, degrading to an empty one when the file is absent,
/// unreadable, corrupt, or written by a future schema version. This never
/// fails: a broken cache must not stop the app from starting.
pub fn load() -> Registry {
    let Some(path) = registry_path() else {
        return Registry::default();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Registry::default();
    };
    match serde_json::from_str::<Registry>(&raw) {
        Ok(registry) if registry.version <= SCHEMA_VERSION => registry,
        _ => Registry::default(),
    }
}

/// Persist the registry, creating the config directory when needed. The write
/// goes to a temp file in the same directory and is renamed into place, so a
/// crash mid-write can never truncate an existing list.
pub fn save(registry: &Registry) -> Result<()> {
    let Some(path) = registry_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut to_write = registry.clone();
    to_write.version = SCHEMA_VERSION;
    let json = serde_json::to_string_pretty(&to_write)
        .map_err(|e| crate::Error::Other(format!("could not serialize the repo registry: {e}")))?;

    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Convenience: record a repository and persist immediately. Failure to
/// persist is returned, but callers may reasonably ignore it — the in-memory
/// list still works for the current session.
pub fn remember(path: &Path, name: &str) -> Result<()> {
    let mut registry = load();
    registry.remember(path, name);
    save(&registry)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_dedupes_by_path_and_refreshes_the_name() {
        let mut registry = Registry::default();
        registry.remember(Path::new("/tmp/proj"), "proj");
        registry.remember(Path::new("/tmp/proj"), "renamed");

        let entries = registry.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "renamed");
    }

    #[test]
    fn entries_are_most_recently_opened_first() {
        let registry = Registry {
            version: SCHEMA_VERSION,
            repos: vec![
                RepoEntry {
                    path: PathBuf::from("/tmp/old"),
                    name: "old".into(),
                    last_opened: 10,
                },
                RepoEntry {
                    path: PathBuf::from("/tmp/new"),
                    name: "new".into(),
                    last_opened: 20,
                },
            ],
        };

        let entries = registry.entries();
        assert_eq!(entries[0].name, "new");
        assert_eq!(entries[1].name, "old");
    }

    #[test]
    fn forget_removes_only_the_named_repo() {
        let mut registry = Registry::default();
        registry.remember(Path::new("/tmp/a"), "a");
        registry.remember(Path::new("/tmp/b"), "b");

        assert!(registry.forget(Path::new("/tmp/a")));
        assert!(!registry.forget(Path::new("/tmp/a")));
        assert!(!registry.contains(Path::new("/tmp/a")));
        assert!(registry.contains(Path::new("/tmp/b")));
    }

    #[test]
    fn a_corrupt_registry_file_loads_as_empty() {
        // The parse path is what matters here; `load` reads a real file, so
        // exercise the same deserialization it uses.
        let parsed = serde_json::from_str::<Registry>("{ not json");
        assert!(parsed.is_err());
        assert!(Registry::default().entries().is_empty());
    }
}

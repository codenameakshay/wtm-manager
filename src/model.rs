use std::path::PathBuf;

use serde::Serialize;

/// Everything `wtm list --json` emits for one worktree. Cheap fields come from
/// the registry; `status` is only populated when status computation is enabled.
#[derive(Debug, Clone, Serialize)]
pub struct WorktreeInfo {
    /// Registry name of the worktree ("main" for the main working tree).
    pub name: String,
    /// Absolute path to the worktree's working directory.
    pub path: PathBuf,
    /// Checked-out branch, or `None` when HEAD is detached or unreadable.
    pub branch: Option<String>,
    /// Abbreviated HEAD commit id, when resolvable.
    pub head: Option<String>,
    pub is_main: bool,
    /// The registry entry exists but its directory is gone from disk.
    pub is_missing: bool,
    pub is_locked: bool,
    /// git considers this entry prunable (`git worktree prune` would drop it).
    pub is_prunable: bool,
    /// Expensive per-worktree status; `None` when skipped via `--no-status`
    /// or uncomputable (e.g. the directory is missing).
    pub status: Option<WorktreeStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeStatus {
    /// Uncommitted changes, including untracked files (ignored files excluded).
    pub dirty: bool,
    /// Exact count of dirty/untracked entries; `0` whenever `dirty` is `false`.
    pub dirty_count: usize,
    /// Commits ahead of upstream; `None` when there is no upstream.
    pub ahead: Option<usize>,
    /// Commits behind upstream; `None` when there is no upstream.
    pub behind: Option<usize>,
    /// The branch has upstream configuration but the upstream ref no longer
    /// exists (e.g. deleted on the remote after a merged PR).
    pub upstream_gone: bool,
    /// The branch tip is an ancestor of (or equal to) the resolved base
    /// (`default_base`, falling back to the main worktree's HEAD).
    pub merged: bool,
}

impl WorktreeInfo {
    /// The label shown to users and matched by `<name>` arguments: the branch
    /// name when one is checked out, otherwise the registry name.
    pub fn display_name(&self) -> &str {
        self.branch.as_deref().unwrap_or(&self.name)
    }
}

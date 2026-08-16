//! The worktree file browser: a lazily-expanding tree over a worktree's
//! working directory, shown in the detail panel's Files tab.
//!
//! Two concerns are deliberately kept apart, in the spirit of
//! [`crate::worktree_list`]/[`crate::detail_panel`]:
//! - [`FileBrowserState`] is pure bookkeeping — which directories are
//!   expanded, what [`crate::data::list_files`] returned for each (or that
//!   it's still loading, or failed), and which file is selected. It never
//!   touches git or the filesystem itself and is fully unit-testable.
//! - [`render_row`] turns one flattened row into an element. It returns a
//!   `Stateful<Div>` with no click handler attached — same convention
//!   [`crate::worktree_list::render_row`] uses — because wiring a click to
//!   `WtmApp::toggle_file_dir`/`select_tree_file` needs `Context<WtmApp>`,
//!   which only `crate::app::chrome` (the owner of this tree's on-screen
//!   assembly) has.
//!
//! The tree expands one level at a time: [`FileBrowserState::dirs_needing_load`]
//! only ever names the root and directories the user has actually opened, so
//! an unexpanded `node_modules` costs exactly one row, never a walk of its
//! contents — the whole reason [`crate::data::list_files`] is one-level-only
//! in the first place.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui::prelude::*;
use gpui::{div, px, AnyElement, Div, Hsla, SharedString, Stateful};

use crate::assets::icons;
use crate::data::{FileEntry, FileStatus};
use crate::theme::Theme;
use crate::ui;

/// Indentation added per tree depth level.
const INDENT: f32 = 14.0;
/// Height of one tree row — slightly tighter than a worktree list row
/// (32px) since a file tree reads better dense.
const ROW_HEIGHT: f32 = 24.0;

/// What's known about one directory's contents: nothing requested yet
/// (absent from [`FileBrowserState`]'s map), a request in flight, a
/// successful listing, or a failure worth telling the user about instead of
/// silently leaving the row inert.
#[derive(Debug, Clone)]
pub enum DirState {
    Loading,
    Loaded(Vec<FileEntry>),
    Error(String),
}

/// Per-worktree file-browser state: which directories are expanded, what
/// has been loaded (or attempted) for each, and which file is selected for
/// the diff pane next to the tree.
///
/// One instance lives per worktree path in `WtmApp::file_trees`, not one
/// shared instance — that is what lets switching the selected worktree away
/// and back leave whatever the user had expanded exactly as they left it,
/// rather than collapsing to just the root the way a single shared tree
/// would if it were rebuilt (or cleared) on every selection change.
#[derive(Default)]
pub struct FileBrowserState {
    expanded: HashSet<PathBuf>,
    dirs: HashMap<PathBuf, DirState>,
    selected_file: Option<PathBuf>,
}

impl FileBrowserState {
    pub fn is_expanded(&self, rel_dir: &Path) -> bool {
        self.expanded.contains(rel_dir)
    }

    /// Flip whether `rel_dir` is expanded, returning the new state (`true`
    /// = now expanded). Collapsing never drops the cached listing — only
    /// which rows are *visible* changes, not what's known; re-expanding the
    /// same directory later is then free (see [`dirs_needing_load`]).
    ///
    /// [`dirs_needing_load`]: FileBrowserState::dirs_needing_load
    pub fn toggle_expanded(&mut self, rel_dir: PathBuf) -> bool {
        if self.expanded.remove(&rel_dir) {
            false
        } else {
            self.expanded.insert(rel_dir);
            true
        }
    }

    pub fn dir_state(&self, rel_dir: &Path) -> Option<&DirState> {
        self.dirs.get(rel_dir)
    }

    pub fn set_loading(&mut self, rel_dir: PathBuf) {
        self.dirs.insert(rel_dir, DirState::Loading);
    }

    pub fn set_loaded(&mut self, rel_dir: PathBuf, entries: Vec<FileEntry>) {
        self.dirs.insert(rel_dir, DirState::Loaded(entries));
    }

    pub fn set_error(&mut self, rel_dir: PathBuf, error: String) {
        self.dirs.insert(rel_dir, DirState::Error(error));
    }

    pub fn selected_file(&self) -> Option<&Path> {
        self.selected_file.as_deref()
    }

    pub fn select_file(&mut self, rel_path: PathBuf) {
        self.selected_file = Some(rel_path);
    }

    /// The root, plus every expanded directory, that has never been
    /// requested — `dirs` holding *any* [`DirState`] (loading, loaded, or
    /// even an error) counts as "already requested" and is left alone here;
    /// a failed listing is retried only by the user collapsing and
    /// re-expanding that row, not automatically on every call, so a
    /// directory this app can't list (permissions, a broken symlink) can't
    /// turn into a background request storm.
    pub fn dirs_needing_load(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let root = PathBuf::new();
        if !self.dirs.contains_key(&root) {
            out.push(root);
        }
        for dir in &self.expanded {
            if !self.dirs.contains_key(dir) {
                out.push(dir.clone());
            }
        }
        out
    }
}

/// One row of the flattened, currently-visible tree, produced by
/// [`visible_rows`]. Borrows from the [`FileBrowserState`] it was built
/// from, so it only lives as long as one render pass — nothing holds these
/// across frames.
pub struct VisibleRow<'a> {
    pub rel_path: &'a Path,
    pub name: &'a str,
    pub is_dir: bool,
    pub depth: usize,
    pub status: Option<FileStatus>,
    /// `Some((expanded, state))` for a directory row; `None` for a file.
    /// `state` is `None` when the directory hasn't been requested yet (a
    /// row can be expanded and still awaiting its first load).
    pub dir: Option<(bool, Option<&'a DirState>)>,
}

/// Depth-first flatten of `state`'s tree starting at the worktree root.
/// Stops descending at any directory that isn't expanded — its children are
/// simply never visited, which is what keeps a huge, never-opened directory
/// free to display (see the module doc).
///
/// Pure and gpui-free, so the traversal/expansion logic is unit-tested
/// directly against a hand-built [`FileBrowserState`] rather than only
/// reachable by driving a live app.
pub fn visible_rows(state: &FileBrowserState) -> Vec<VisibleRow<'_>> {
    let mut out = Vec::new();
    if let Some(DirState::Loaded(entries)) = state.dir_state(Path::new("")) {
        push_entries(state, entries, 0, &mut out);
    }
    out
}

fn push_entries<'a>(
    state: &'a FileBrowserState,
    entries: &'a [FileEntry],
    depth: usize,
    out: &mut Vec<VisibleRow<'a>>,
) {
    for entry in entries {
        if entry.is_dir {
            let expanded = state.is_expanded(&entry.rel_path);
            out.push(VisibleRow {
                rel_path: &entry.rel_path,
                name: &entry.name,
                is_dir: true,
                depth,
                status: entry.status,
                dir: Some((expanded, state.dir_state(&entry.rel_path))),
            });
            if expanded {
                if let Some(DirState::Loaded(children)) = state.dir_state(&entry.rel_path) {
                    push_entries(state, children, depth + 1, out);
                }
            }
        } else {
            out.push(VisibleRow {
                rel_path: &entry.rel_path,
                name: &entry.name,
                is_dir: false,
                depth,
                status: entry.status,
                dir: None,
            });
        }
    }
}

/// Map a [`FileStatus`] to the same semantic colors `worktree_list`/
/// `detail_panel` already use for a worktree's own status pills — modified
/// reads as "dirty" (warning), added as success, deleted/conflicted as
/// danger, untracked as the neutral `text_tertiary` a new, not-yet-tracked
/// file deserves rather than an alarm color. `Renamed` has no existing
/// worktree-level precedent to match (there is no per-worktree "renamed"
/// status); `info` is used for it here as the same "structurally different,
/// not necessarily bad" register `detail_panel` already gives "behind".
///
/// Centralized here rather than duplicated at every call site that shows a
/// [`FileStatus`] — the tree rows, the diff header pill, the Changes tab
/// list — so all of them read as one vocabulary; see `diff_view`'s re-use
/// of this function.
pub fn status_color(status: FileStatus, theme: &Theme) -> Hsla {
    match status {
        FileStatus::Modified => theme.warning,
        FileStatus::Added => theme.success,
        FileStatus::Deleted => theme.danger,
        FileStatus::Renamed => theme.info,
        FileStatus::Untracked => theme.text_tertiary,
        FileStatus::Conflicted => theme.danger,
    }
}

/// Short, lowercase label for a [`FileStatus`], for the diff header pill and
/// the Changes tab's per-file heading.
pub fn status_label(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Modified => "modified",
        FileStatus::Added => "added",
        FileStatus::Deleted => "deleted",
        FileStatus::Renamed => "renamed",
        FileStatus::Untracked => "untracked",
        FileStatus::Conflicted => "conflicted",
    }
}

/// One tree row. Returns a stateful element with no click handler attached
/// — see the module doc for why the caller (`crate::app::chrome`) attaches
/// one, the same split `worktree_list::render_row` uses for the main list.
pub fn render_row(
    row: &VisibleRow<'_>,
    selected_file: Option<&Path>,
    theme: &Theme,
) -> Stateful<Div> {
    let id = SharedString::from(format!("file-row:{}", row.rel_path.display()));
    let is_selected = !row.is_dir && selected_file == Some(row.rel_path);
    let expanded = row.dir.is_some_and(|(expanded, _)| expanded);
    let name_color = match row.status {
        Some(status) => status_color(status, theme),
        None if row.is_dir => theme.text,
        None => theme.text_secondary,
    };

    // A directory that's open but still loading (or failed) gets a small
    // trailing note so that state is never silently invisible — an empty
    // row with a spinner-less wait, or worse, an open folder that just
    // never shows anything, would both read as broken rather than pending.
    let trailing: Option<AnyElement> = if row.is_dir && expanded {
        match row.dir.and_then(|(_, s)| s) {
            Some(DirState::Loading) => Some(
                div()
                    .flex_none()
                    .text_size(px(10.5))
                    .text_color(theme.text_ghost)
                    .child("loading…")
                    .into_any_element(),
            ),
            Some(DirState::Error(e)) => Some(
                div()
                    .flex_none()
                    .max_w(px(140.0))
                    .truncate()
                    .text_size(px(10.5))
                    .text_color(theme.danger)
                    .child(format!("error: {e}"))
                    .into_any_element(),
            ),
            _ => None,
        }
    } else {
        None
    };

    div()
        .id(id)
        .h(px(ROW_HEIGHT))
        .w_full()
        .min_w_0()
        .pl(px(8.0 + row.depth as f32 * INDENT))
        .pr(px(8.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .rounded(px(ui::RADIUS))
        .cursor_default()
        .when(is_selected, |d| d.bg(theme.item_selected))
        .when(!is_selected, |d| d.hover(|s| s.bg(theme.item_wash)))
        .child(
            // Fixed-width disclosure glyph column: a directory shows
            // ▸/▾ for collapsed/expanded, a file shows nothing, but the
            // column is always reserved so file rows still line up under
            // their siblings' names instead of drifting left.
            div()
                .flex_none()
                .w(px(10.0))
                .text_size(px(9.0))
                .text_color(theme.text_ghost)
                .child(if !row.is_dir {
                    ""
                } else if expanded {
                    "▾"
                } else {
                    "▸"
                }),
        )
        .when(row.is_dir, |d| {
            d.child(ui::icon(icons::FOLDER, 12.0, theme.text_tertiary))
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(12.5))
                .text_color(name_color)
                .child(row.name.to_string()),
        )
        .when_some(trailing, |d, el| d.child(el))
}

/// The Files tab's selected-file diff pane state — loaded in the background
/// by `crate::app::loading` and kept in `WtmApp::selected_file_diff`, guarded
/// the same generation-counter way `WtmApp::details` already is (see that
/// module).
pub enum SelectedFileDiff {
    /// No file has been clicked in the tree yet.
    Unselected,
    Loading,
    /// The file has no uncommitted changes — deliberately distinct from
    /// `Unselected` so the panel can say so plainly instead of showing an
    /// empty box that looks like it just hasn't loaded yet.
    NoChanges,
    Changed(crate::data::FileDiff),
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool, status: Option<FileStatus>) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            rel_path: PathBuf::from(name),
            is_dir,
            status,
        }
    }

    fn nested_entry(parent: &str, name: &str, is_dir: bool) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            rel_path: Path::new(parent).join(name),
            is_dir,
            status: None,
        }
    }

    // ---------------- FileBrowserState bookkeeping ----------------

    #[test]
    fn new_state_needs_only_the_root_loaded() {
        let state = FileBrowserState::default();
        assert_eq!(state.dirs_needing_load(), vec![PathBuf::new()]);
    }

    #[test]
    fn toggle_expanded_flips_and_reports_new_state() {
        let mut state = FileBrowserState::default();
        let dir = PathBuf::from("src");
        assert!(!state.is_expanded(&dir));
        assert!(state.toggle_expanded(dir.clone()));
        assert!(state.is_expanded(&dir));
        assert!(!state.toggle_expanded(dir.clone()));
        assert!(!state.is_expanded(&dir));
    }

    #[test]
    fn collapsing_keeps_the_cached_listing() {
        let mut state = FileBrowserState::default();
        let dir = PathBuf::from("src");
        state.toggle_expanded(dir.clone());
        state.set_loaded(dir.clone(), vec![entry("lib.rs", false, None)]);
        state.toggle_expanded(dir.clone()); // collapse
        assert!(matches!(state.dir_state(&dir), Some(DirState::Loaded(_))));
        // Not re-requested just because it's collapsed and not currently
        // expanded (`dirs_needing_load` only reports the root here, which
        // has never been loaded in this test).
        assert!(!state.dirs_needing_load().contains(&dir));
    }

    #[test]
    fn dirs_needing_load_covers_root_and_expanded_unrequested_dirs() {
        let mut state = FileBrowserState::default();
        state.set_loaded(PathBuf::new(), vec![]);
        let a = PathBuf::from("a");
        let b = PathBuf::from("b");
        state.toggle_expanded(a.clone());
        state.toggle_expanded(b.clone());
        state.set_loading(a.clone());

        let needing = state.dirs_needing_load();
        assert_eq!(needing, vec![b.clone()]);
    }

    #[test]
    fn dirs_needing_load_skips_dirs_with_any_recorded_state() {
        let mut state = FileBrowserState::default();
        state.set_loaded(PathBuf::new(), vec![]);
        let failed = PathBuf::from("broken");
        state.toggle_expanded(failed.clone());
        state.set_error(failed.clone(), "permission denied".to_string());
        assert!(state.dirs_needing_load().is_empty());
    }

    #[test]
    fn select_file_records_the_choice() {
        let mut state = FileBrowserState::default();
        assert_eq!(state.selected_file(), None);
        state.select_file(PathBuf::from("README.md"));
        assert_eq!(state.selected_file(), Some(Path::new("README.md")));
    }

    // ---------------- visible_rows ----------------

    #[test]
    fn visible_rows_is_empty_before_root_loads() {
        let state = FileBrowserState::default();
        assert!(visible_rows(&state).is_empty());
    }

    #[test]
    fn visible_rows_lists_root_entries_in_listing_order() {
        let mut state = FileBrowserState::default();
        state.set_loaded(
            PathBuf::new(),
            vec![
                entry("src", true, None),
                entry("Cargo.toml", false, Some(FileStatus::Modified)),
            ],
        );
        let rows = visible_rows(&state);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "src");
        assert_eq!(rows[0].depth, 0);
        assert!(rows[0].is_dir);
        assert_eq!(rows[1].name, "Cargo.toml");
        assert_eq!(rows[1].status, Some(FileStatus::Modified));
    }

    #[test]
    fn collapsed_directory_hides_its_children_entirely() {
        let mut state = FileBrowserState::default();
        state.set_loaded(PathBuf::new(), vec![entry("src", true, None)]);
        // "src" is loaded (as if previously expanded) but not currently
        // expanded — its children must not appear.
        state.set_loaded(
            PathBuf::from("src"),
            vec![nested_entry("src", "lib.rs", false)],
        );
        let rows = visible_rows(&state);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "src");
    }

    #[test]
    fn expanded_directory_shows_children_indented_one_level() {
        let mut state = FileBrowserState::default();
        state.set_loaded(PathBuf::new(), vec![entry("src", true, None)]);
        state.toggle_expanded(PathBuf::from("src"));
        state.set_loaded(
            PathBuf::from("src"),
            vec![nested_entry("src", "lib.rs", false)],
        );

        let rows = visible_rows(&state);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "src");
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[1].name, "lib.rs");
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[1].rel_path, Path::new("src/lib.rs"));
    }

    #[test]
    fn expanded_directory_awaiting_its_listing_contributes_no_children_yet() {
        let mut state = FileBrowserState::default();
        state.set_loaded(PathBuf::new(), vec![entry("src", true, None)]);
        state.toggle_expanded(PathBuf::from("src"));
        state.set_loading(PathBuf::from("src"));

        let rows = visible_rows(&state);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "src");
        assert!(matches!(rows[0].dir, Some((true, Some(DirState::Loading)))));
    }

    // ---------------- status_color / status_label ----------------

    #[test]
    fn status_color_matches_worktree_lists_established_meanings() {
        let theme = Theme::dark();
        assert_eq!(status_color(FileStatus::Modified, &theme), theme.warning);
        assert_eq!(status_color(FileStatus::Added, &theme), theme.success);
        assert_eq!(status_color(FileStatus::Deleted, &theme), theme.danger);
        assert_eq!(status_color(FileStatus::Conflicted, &theme), theme.danger);
        assert_eq!(
            status_color(FileStatus::Untracked, &theme),
            theme.text_tertiary
        );
    }

    #[test]
    fn status_label_is_lowercase_and_distinct() {
        let labels = [
            FileStatus::Modified,
            FileStatus::Added,
            FileStatus::Deleted,
            FileStatus::Renamed,
            FileStatus::Untracked,
            FileStatus::Conflicted,
        ]
        .map(status_label);
        let mut sorted = labels.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            labels.len(),
            "labels must be distinct: {labels:?}"
        );
        for label in labels {
            assert_eq!(label, label.to_lowercase());
        }
    }
}

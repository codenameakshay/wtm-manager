//! Pure TUI state machine: model, messages, and effects.
//!
//! `App::update` is the single state-transition function and performs NO
//! I/O: it consumes a [`Msg`] and returns [`Effect`] values describing what
//! the runtime loop (in the parent module) must execute. This keeps every
//! transition unit-testable without a terminal, threads, or a repository.
//!
//! No git or business logic lives here either: create/remove/prune are
//! dispatched as effects to the shared cores in `crate::commands`, and
//! prune candidate selection calls `commands::prune::{candidates,
//! selection_candidates}` (pure when `verbose` is off).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::commands::prune::{self, PruneCandidate};
use crate::model::WorktreeInfo;
use crate::worktree::WorktreeDetails;

/// Everything that can happen to the model: terminal input plus results
/// delivered by the runtime (background loads, executed actions).
pub(crate) enum Msg {
    Key(KeyEvent),
    /// A worktree listing finished loading (fast pass or full-status pass).
    RowsLoaded {
        generation: u64,
        rows: Vec<WorktreeInfo>,
        with_status: bool,
    },
    RowsFailed {
        generation: u64,
        with_status: bool,
        text: String,
    },
    /// Detail-pane data for one worktree finished loading.
    Details {
        generation: u64,
        path: PathBuf,
        details: Option<WorktreeDetails>,
    },
    /// A side-effectful action finished; show its outcome and optionally
    /// trigger a refresh.
    ActionOutcome {
        text: String,
        error: bool,
        refresh: bool,
    },
}

/// Side effects the runtime executes on behalf of `update`.
#[derive(Debug)]
pub(crate) enum Effect {
    /// Load the worktree list on a background thread.
    LoadRows { generation: u64, with_status: bool },
    /// Load detail-pane data for one worktree on a background thread.
    LoadDetails { generation: u64, path: PathBuf },
    /// Write the cd file for `path` and quit.
    Switch { path: PathBuf },
    /// Run the shared create core (base is never empty; "HEAD" when the
    /// form's base field was cleared).
    Create { branch: String, base: String },
    /// Run the shared safety-checked remove core.
    Remove {
        info: Box<WorktreeInfo>,
        force: bool,
    },
    /// Run the shared prune execution over pre-confirmed candidates.
    Prune {
        candidates: Vec<PruneCandidate>,
        force: bool,
    },
    /// Open the worktree in the configured editor.
    OpenEditor { path: PathBuf },
    /// Suspend the TUI and run a shell command inside the worktree.
    RunCommand { path: PathBuf, command: String },
    /// Copy the path to the system clipboard.
    CopyPath { path: PathBuf },
    /// Leave the TUI without switching.
    Quit,
}

/// Which create-form input currently has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateField {
    Branch,
    Base,
}

/// Modal state. At most one overlay is open; Esc always closes it before
/// anything else happens (see `on_key`).
pub(crate) enum Overlay {
    None,
    Help,
    /// A dismissible informational modal (e.g. a rejected action). Any key
    /// closes it; crucially it captures that key so it never falls through to
    /// a normal-mode binding — notably `Enter`, which would otherwise switch
    /// worktree and quit the whole TUI.
    Notice {
        text: String,
    },
    ConfirmRemove {
        info: WorktreeInfo,
        /// Explicit force choice; required before confirming a dirty
        /// worktree.
        force: bool,
        dirty: bool,
    },
    ConfirmPrune {
        candidates: Vec<PruneCandidate>,
        force: bool,
        unsafe_count: usize,
    },
    Create {
        branch: String,
        base: String,
        field: CreateField,
    },
    Command {
        input: String,
    },
}

/// One-line feedback shown in the footer area.
pub(crate) struct Message {
    pub text: String,
    pub error: bool,
}

/// The whole TUI model. Views are pure functions of this struct.
pub(crate) struct App {
    /// All worktrees, in listing order (main first).
    pub rows: Vec<WorktreeInfo>,
    /// Indices into `rows` matching the current filter, in order.
    pub filtered: Vec<usize>,
    /// Cursor position within `filtered`.
    pub cursor: usize,
    /// Multi-selected worktrees, keyed by path (stable across refreshes).
    pub marked: BTreeSet<PathBuf>,
    /// Current fuzzy filter text (empty = no filter).
    pub filter: String,
    /// The filter input is being edited (`/` pressed, Enter/Esc not yet).
    pub filter_editing: bool,
    pub overlay: Overlay,
    /// Detail-pane cache, keyed by worktree path.
    pub details: HashMap<PathBuf, Option<WorktreeDetails>>,
    /// Paths with an in-flight detail load (dedupes requests).
    requested: BTreeSet<PathBuf>,
    detail_generations: HashMap<PathBuf, u64>,
    pub message: Option<Message>,
    /// True until a with-status listing has arrived (drives the "loading
    /// status…" indicator).
    pub status_loading: bool,
    next_generation: u64,
    rows_generation: u64,
    /// Configured base for new branches (prefills the create form).
    default_base: Option<String>,
    /// Branches prune must never touch.
    protected: Vec<String>,
}

impl App {
    pub fn new(default_base: Option<String>, protected: Vec<String>) -> Self {
        App {
            rows: Vec::new(),
            filtered: Vec::new(),
            cursor: 0,
            marked: BTreeSet::new(),
            filter: String::new(),
            filter_editing: false,
            overlay: Overlay::None,
            details: HashMap::new(),
            requested: BTreeSet::new(),
            detail_generations: HashMap::new(),
            message: None,
            status_loading: true,
            next_generation: 0,
            rows_generation: 0,
            default_base,
            protected,
        }
    }

    /// The worktree under the cursor, if any row is visible.
    pub fn selected(&self) -> Option<&WorktreeInfo> {
        self.filtered.get(self.cursor).map(|&i| &self.rows[i])
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.selected().map(|i| i.path.clone())
    }

    /// The single state-transition function. Pure: no I/O, only returned
    /// effects.
    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    return Vec::new();
                }
                self.on_key(key)
            }
            Msg::RowsLoaded {
                generation,
                rows,
                with_status,
            } if generation == self.rows_generation => self.on_rows(rows, with_status),
            Msg::RowsLoaded { .. } => Vec::new(),
            Msg::RowsFailed {
                generation,
                with_status,
                text,
            } if generation == self.rows_generation => {
                if with_status {
                    self.status_loading = false;
                }
                self.message = Some(Message { text, error: true });
                Vec::new()
            }
            Msg::RowsFailed { .. } => Vec::new(),
            Msg::Details {
                generation,
                path,
                details,
            } if self.detail_generations.get(&path) == Some(&generation) => {
                self.requested.remove(&path);
                self.details.insert(path, details);
                Vec::new()
            }
            Msg::Details { .. } => Vec::new(),
            Msg::ActionOutcome {
                text,
                error,
                refresh,
            } => {
                self.message = Some(Message { text, error });
                if refresh {
                    vec![self.request_rows(true)]
                } else {
                    Vec::new()
                }
            }
        }
    }

    pub(crate) fn request_rows(&mut self, with_status: bool) -> Effect {
        self.next_generation += 1;
        self.rows_generation = self.next_generation;
        if with_status {
            self.status_loading = true;
        }
        Effect::LoadRows {
            generation: self.rows_generation,
            with_status,
        }
    }

    fn on_rows(&mut self, rows: Vec<WorktreeInfo>, with_status: bool) -> Vec<Effect> {
        let keep = self.selected_path();
        self.rows = rows;
        if with_status {
            self.status_loading = false;
            // Statuses changed, so cached details may be stale.
            self.details.clear();
            self.requested.clear();
            self.detail_generations.clear();
        }
        let existing: BTreeSet<PathBuf> = self.rows.iter().map(|i| i.path.clone()).collect();
        self.marked.retain(|p| existing.contains(p));
        self.apply_filter(keep.as_deref());
        self.selection_effects()
    }

    /// Recompute `filtered` and restore the cursor to `keep` when that row
    /// still matches, clamping otherwise.
    fn apply_filter(&mut self, keep: Option<&Path>) {
        self.filtered = (0..self.rows.len())
            .filter(|&i| row_matches(&self.rows[i], &self.filter))
            .collect();
        self.cursor = keep
            .and_then(|p| self.filtered.iter().position(|&i| self.rows[i].path == p))
            .unwrap_or_else(|| self.cursor.min(self.filtered.len().saturating_sub(1)));
    }

    /// Request detail data for the current selection when not yet cached.
    fn selection_effects(&mut self) -> Vec<Effect> {
        let Some(info) = self.selected() else {
            return Vec::new();
        };
        if info.is_missing
            || self.details.contains_key(&info.path)
            || self.requested.contains(&info.path)
        {
            return Vec::new();
        }
        let path = info.path.clone();
        self.requested.insert(path.clone());
        self.next_generation += 1;
        let generation = self.next_generation;
        self.detail_generations.insert(path.clone(), generation);
        vec![Effect::LoadDetails { generation, path }]
    }

    fn on_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        // Ctrl-C quits from anywhere (raw mode swallows the signal).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return vec![Effect::Quit];
        }
        self.message = None;
        if !matches!(self.overlay, Overlay::None) {
            return self.on_overlay_key(key);
        }
        if self.filter_editing {
            return self.on_filter_key(key);
        }
        self.on_normal_key(key)
    }

    fn on_overlay_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match &mut self.overlay {
            Overlay::None => Vec::new(),
            Overlay::Help => {
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::Enter
                ) {
                    self.overlay = Overlay::None;
                }
                Vec::new()
            }
            // A notice swallows EVERY key (nothing leaks to normal mode) and
            // closes only on an explicit acknowledge/cancel key. This is what
            // makes a burst like `d f Enter` safe when `d` opened the notice
            // (cursor on the main worktree): `f` is swallowed and keeps the
            // modal up, and the trailing Enter dismisses it here instead of
            // falling through to the normal-mode switch-and-quit binding.
            Overlay::Notice { .. } => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q')) {
                    self.overlay = Overlay::None;
                }
                Vec::new()
            }
            Overlay::ConfirmRemove { info, force, dirty } => match key.code {
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.overlay = Overlay::None;
                    Vec::new()
                }
                KeyCode::Char('f') => {
                    *force = !*force;
                    Vec::new()
                }
                KeyCode::Enter | KeyCode::Char('y') => {
                    if *dirty && !*force {
                        self.message = Some(Message {
                            text: "worktree has uncommitted changes — press f to toggle force"
                                .to_string(),
                            error: true,
                        });
                        return Vec::new();
                    }
                    let effect = Effect::Remove {
                        info: Box::new(info.clone()),
                        force: *force,
                    };
                    self.overlay = Overlay::None;
                    vec![effect]
                }
                _ => Vec::new(),
            },
            Overlay::ConfirmPrune {
                candidates,
                force,
                unsafe_count,
            } => match key.code {
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.overlay = Overlay::None;
                    Vec::new()
                }
                KeyCode::Char('f') => {
                    *force = !*force;
                    Vec::new()
                }
                KeyCode::Enter | KeyCode::Char('y') => {
                    if *unsafe_count > 0 && !*force {
                        self.message = Some(Message {
                            text: format!(
                                "{unsafe_count} selected worktree(s) may be dirty — press f to toggle force"
                            ),
                            error: true,
                        });
                        return Vec::new();
                    }
                    let effect = Effect::Prune {
                        candidates: std::mem::take(candidates),
                        force: *force,
                    };
                    self.overlay = Overlay::None;
                    vec![effect]
                }
                _ => Vec::new(),
            },
            Overlay::Create {
                branch,
                base,
                field,
            } => match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Vec::new()
                }
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Down | KeyCode::Up => {
                    *field = match field {
                        CreateField::Branch => CreateField::Base,
                        CreateField::Base => CreateField::Branch,
                    };
                    Vec::new()
                }
                KeyCode::Backspace => {
                    match field {
                        CreateField::Branch => branch.pop(),
                        CreateField::Base => base.pop(),
                    };
                    Vec::new()
                }
                KeyCode::Char(c) => {
                    match field {
                        CreateField::Branch => branch.push(c),
                        CreateField::Base => base.push(c),
                    }
                    Vec::new()
                }
                KeyCode::Enter => {
                    let branch = branch.trim().to_string();
                    if branch.is_empty() {
                        self.message = Some(Message {
                            text: "branch name is required".to_string(),
                            error: true,
                        });
                        return Vec::new();
                    }
                    // Empty base = HEAD (the user cleared the prefilled
                    // default_base on purpose).
                    let base = base.trim().to_string();
                    let base = if base.is_empty() {
                        "HEAD".to_string()
                    } else {
                        base
                    };
                    self.overlay = Overlay::None;
                    vec![Effect::Create { branch, base }]
                }
                _ => Vec::new(),
            },
            Overlay::Command { input } => match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Vec::new()
                }
                KeyCode::Backspace => {
                    input.pop();
                    Vec::new()
                }
                KeyCode::Char(c) => {
                    input.push(c);
                    Vec::new()
                }
                KeyCode::Enter => {
                    let command = input.trim().to_string();
                    self.overlay = Overlay::None;
                    if command.is_empty() {
                        return Vec::new();
                    }
                    match self.selected_existing() {
                        Ok(path) => vec![Effect::RunCommand { path, command }],
                        Err(effects) => effects,
                    }
                }
                _ => Vec::new(),
            },
        }
    }

    fn on_filter_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Esc => {
                self.filter.clear();
                self.filter_editing = false;
                let keep = self.selected_path();
                self.apply_filter(keep.as_deref());
                self.selection_effects()
            }
            KeyCode::Enter => {
                self.filter_editing = false;
                Vec::new()
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.refilter()
            }
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.refilter()
            }
            KeyCode::Down => self.move_cursor(1),
            KeyCode::Up => self.move_cursor(-1),
            _ => Vec::new(),
        }
    }

    fn refilter(&mut self) -> Vec<Effect> {
        let keep = self.selected_path();
        self.apply_filter(keep.as_deref());
        self.selection_effects()
    }

    fn on_normal_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Char('q') => vec![Effect::Quit],
            KeyCode::Esc => {
                // Esc precedence: clear an active filter before quitting.
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.refilter()
                } else {
                    vec![Effect::Quit]
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Char('g') | KeyCode::Home => {
                self.cursor = 0;
                self.selection_effects()
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.cursor = self.filtered.len().saturating_sub(1);
                self.selection_effects()
            }
            KeyCode::Enter => match self.selected_existing() {
                Ok(path) => vec![Effect::Switch { path }],
                Err(effects) => effects,
            },
            KeyCode::Char(' ') => {
                if let Some(path) = self.selected_path() {
                    if !self.marked.remove(&path) {
                        self.marked.insert(path);
                    }
                }
                Vec::new()
            }
            KeyCode::Char('n') => {
                self.overlay = Overlay::Create {
                    branch: String::new(),
                    base: self.default_base.clone().unwrap_or_default(),
                    field: CreateField::Branch,
                };
                Vec::new()
            }
            KeyCode::Char('d') => {
                let Some(info) = self.selected().cloned() else {
                    return Vec::new();
                };
                if info.is_main {
                    // A modal (not a footer note): the follow-up keystroke a
                    // user types when they expected the remove dialog — most
                    // dangerously Enter — is captured here instead of leaking
                    // into normal mode, where Enter would switch and quit.
                    self.overlay = Overlay::Notice {
                        text: "cannot remove the main worktree".to_string(),
                    };
                    return Vec::new();
                }
                let dirty = info.status.as_ref().is_some_and(|s| s.dirty);
                self.overlay = Overlay::ConfirmRemove {
                    info,
                    force: false,
                    dirty,
                };
                Vec::new()
            }
            KeyCode::Char('p') => {
                if self.status_loading {
                    self.message = Some(Message {
                        text: "wait for status loading to finish before pruning".to_string(),
                        error: true,
                    });
                    return Vec::new();
                }
                let candidates = if self.marked.is_empty() {
                    prune::candidates(self.rows.clone(), &self.protected, true, true, false)
                } else {
                    let selection: Vec<WorktreeInfo> = self
                        .rows
                        .iter()
                        .filter(|i| self.marked.contains(&i.path))
                        .cloned()
                        .collect();
                    prune::selection_candidates(selection, &self.protected)
                };
                if candidates.is_empty() {
                    self.message = Some(Message {
                        text: "nothing to prune".to_string(),
                        error: false,
                    });
                } else {
                    let unsafe_count = candidates
                        .iter()
                        .filter(|candidate| {
                            !candidate.info.is_missing
                                && candidate
                                    .info
                                    .status
                                    .as_ref()
                                    .is_none_or(|status| status.dirty)
                        })
                        .count();
                    self.overlay = Overlay::ConfirmPrune {
                        candidates,
                        force: false,
                        unsafe_count,
                    };
                }
                Vec::new()
            }
            KeyCode::Char('o') => match self.selected_existing() {
                Ok(path) => vec![Effect::OpenEditor { path }],
                Err(effects) => effects,
            },
            KeyCode::Char('x') => {
                self.overlay = Overlay::Command {
                    input: String::new(),
                };
                Vec::new()
            }
            KeyCode::Char('y') => match self.selected_existing() {
                Ok(path) => vec![Effect::CopyPath { path }],
                Err(effects) => effects,
            },
            KeyCode::Char('/') => {
                self.filter_editing = true;
                Vec::new()
            }
            KeyCode::Char('r') => {
                vec![self.request_rows(true)]
            }
            KeyCode::Char('?') => {
                self.overlay = Overlay::Help;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn move_cursor(&mut self, delta: isize) -> Vec<Effect> {
        if self.filtered.is_empty() {
            return Vec::new();
        }
        let last = self.filtered.len() - 1;
        let next = self.cursor.saturating_add_signed(delta).min(last);
        if next == self.cursor {
            return Vec::new();
        }
        self.cursor = next;
        self.selection_effects()
    }

    /// The selected worktree's path, refusing missing directories with a
    /// footer message (the Err carries the — empty — effect list).
    fn selected_existing(&mut self) -> std::result::Result<PathBuf, Vec<Effect>> {
        match self.selected() {
            Some(info) if info.is_missing => {
                self.message = Some(Message {
                    text: format!("'{}': directory is missing", info.display_name()),
                    error: true,
                });
                Err(Vec::new())
            }
            Some(info) => Ok(info.path.clone()),
            None => Err(Vec::new()),
        }
    }
}

/// Case-insensitive subsequence match of the filter against the row's
/// display name, registry name, and path.
fn row_matches(info: &WorktreeInfo, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let filter = filter.to_lowercase();
    [
        info.display_name().to_lowercase(),
        info.name.to_lowercase(),
        info.path.to_string_lossy().to_lowercase(),
    ]
    .iter()
    .any(|hay| is_subsequence(&filter, hay))
}

/// Is `needle` a subsequence of `hay` (all chars appear, in order)?
fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut chars = hay.chars();
    needle.chars().all(|n| chars.any(|h| h == n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WorktreeStatus;

    fn key(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn info(name: &str, is_main: bool) -> WorktreeInfo {
        WorktreeInfo {
            name: name.to_string(),
            path: PathBuf::from(format!("/wt/{name}")),
            branch: Some(name.to_string()),
            head: Some("abc1234".to_string()),
            is_main,
            is_missing: false,
            is_locked: false,
            is_prunable: false,
            status: None,
        }
    }

    fn with_status(mut i: WorktreeInfo, dirty: bool, merged: bool) -> WorktreeInfo {
        i.status = Some(WorktreeStatus {
            dirty,
            ahead: Some(1),
            behind: Some(0),
            upstream_gone: false,
            merged,
        });
        i
    }

    fn app_with(rows: Vec<WorktreeInfo>) -> App {
        let mut app = App::new(Some("origin/main".to_string()), vec!["main".to_string()]);
        app.update(Msg::RowsLoaded {
            generation: 0,
            rows,
            with_status: true,
        });
        app
    }

    fn three_row_app() -> App {
        app_with(vec![
            info("main", true),
            with_status(info("feat-a", false), false, false),
            info("feat-b", false),
        ])
    }

    #[test]
    fn navigation_moves_and_clamps() {
        let mut app = three_row_app();
        assert_eq!(app.cursor, 0);
        app.update(key(KeyCode::Char('k'))); // clamped at top
        assert_eq!(app.cursor, 0);
        app.update(key(KeyCode::Char('j')));
        assert_eq!(app.cursor, 1);
        app.update(key(KeyCode::Char('G')));
        assert_eq!(app.cursor, 2);
        app.update(key(KeyCode::Char('j'))); // clamped at bottom
        assert_eq!(app.cursor, 2);
        app.update(key(KeyCode::Char('g')));
        assert_eq!(app.cursor, 0);
        app.update(key(KeyCode::Down));
        assert_eq!(app.cursor, 1);
        app.update(key(KeyCode::Up));
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn moving_selection_requests_details_once() {
        let mut app = three_row_app();
        let fx = app.update(key(KeyCode::Char('j')));
        match &fx[..] {
            [Effect::LoadDetails { path, .. }] => assert_eq!(path, Path::new("/wt/feat-a")),
            other => panic!("expected LoadDetails, got {other:?}"),
        }
        // Moving away and back does not re-request.
        app.update(key(KeyCode::Char('k')));
        let fx = app.update(key(KeyCode::Char('j')));
        assert!(fx.is_empty());
    }

    #[test]
    fn multi_select_toggles_and_survives_reload() {
        let mut app = three_row_app();
        app.update(key(KeyCode::Char('j')));
        app.update(key(KeyCode::Char(' ')));
        assert!(app.marked.contains(Path::new("/wt/feat-a")));
        app.update(key(KeyCode::Char(' ')));
        assert!(app.marked.is_empty(), "second toggle clears the mark");

        // Marks on rows that vanish from a reload are dropped.
        app.update(key(KeyCode::Char(' ')));
        app.update(Msg::RowsLoaded {
            generation: 0,
            rows: vec![info("main", true), info("feat-b", false)],
            with_status: true,
        });
        assert!(app.marked.is_empty());
    }

    #[test]
    fn filter_narrows_follows_selection_and_esc_clears() {
        let mut app = three_row_app();
        app.update(key(KeyCode::Char('j'))); // select feat-a
        app.update(key(KeyCode::Char('/')));
        assert!(app.filter_editing);
        app.update(key(KeyCode::Char('f')));
        app.update(key(KeyCode::Char('a')));
        // Subsequence "fa" matches both feat-a and feat-b.
        assert_eq!(app.filtered.len(), 2);
        // Selection followed feat-a into the narrowed list.
        assert_eq!(app.selected().unwrap().name, "feat-a");
        app.update(key(KeyCode::Char('t')));
        app.update(key(KeyCode::Char('a')));
        // "fata" matches only feat-a.
        assert_eq!(app.filtered.len(), 1);
        assert_eq!(app.selected().unwrap().name, "feat-a");

        // Esc while editing clears the filter entirely.
        app.update(key(KeyCode::Esc));
        assert!(!app.filter_editing);
        assert!(app.filter.is_empty());
        assert_eq!(app.filtered.len(), 3);
        assert_eq!(app.selected().unwrap().name, "feat-a");
    }

    #[test]
    fn esc_precedence_overlay_then_filter_then_quit() {
        let mut app = three_row_app();
        // Accepted (non-editing) filter + open overlay.
        app.update(key(KeyCode::Char('/')));
        app.update(key(KeyCode::Char('f')));
        app.update(key(KeyCode::Enter));
        assert!(!app.filter.is_empty() && !app.filter_editing);
        app.update(key(KeyCode::Char('?')));
        assert!(matches!(app.overlay, Overlay::Help));

        // 1st Esc: closes the overlay.
        let fx = app.update(key(KeyCode::Esc));
        assert!(fx.is_empty());
        assert!(matches!(app.overlay, Overlay::None));
        // 2nd Esc: clears the filter.
        let fx = app.update(key(KeyCode::Esc));
        assert!(fx.is_empty());
        assert!(app.filter.is_empty());
        // 3rd Esc: quits.
        let fx = app.update(key(KeyCode::Esc));
        assert!(matches!(&fx[..], [Effect::Quit]));
    }

    #[test]
    fn q_quits_and_enter_switches() {
        let mut app = three_row_app();
        assert!(matches!(
            &app.update(key(KeyCode::Char('q')))[..],
            [Effect::Quit]
        ));
        app.update(key(KeyCode::Char('j')));
        match &app.update(key(KeyCode::Enter))[..] {
            [Effect::Switch { path }] => assert_eq!(path, Path::new("/wt/feat-a")),
            other => panic!("expected Switch, got {other:?}"),
        }
    }

    #[test]
    fn remove_refuses_main_and_requires_force_when_dirty() {
        let mut app = app_with(vec![
            info("main", true),
            with_status(info("feat", false), true, false),
        ]);
        // On main: a dismissible notice modal opens (not a passive footer
        // note), and dismissing it returns to a clean state.
        app.update(key(KeyCode::Char('d')));
        assert!(matches!(app.overlay, Overlay::Notice { .. }));
        app.update(key(KeyCode::Esc));
        assert!(matches!(app.overlay, Overlay::None));

        // On the dirty worktree: modal opens, Enter alone is refused.
        app.update(key(KeyCode::Char('j')));
        app.update(key(KeyCode::Char('d')));
        assert!(matches!(app.overlay, Overlay::ConfirmRemove { .. }));
        let fx = app.update(key(KeyCode::Enter));
        assert!(fx.is_empty(), "dirty removal without force must not fire");
        assert!(matches!(app.overlay, Overlay::ConfirmRemove { .. }));

        // Toggle force, then confirm.
        app.update(key(KeyCode::Char('f')));
        let fx = app.update(key(KeyCode::Enter));
        assert_eq!(fx.len(), 1);
        match &fx[0] {
            Effect::Remove { info, force } => {
                assert_eq!(info.name, "feat");
                assert!(force);
            }
            other => panic!("expected Remove, got {other:?}"),
        }
        assert!(matches!(app.overlay, Overlay::None));
    }

    /// Regression: pressing `d f Enter` while the cursor is on the main
    /// worktree (a common mistake when you have a single worktree and never
    /// moved off `main`) must NOT silently switch-and-quit the TUI. `d` opens
    /// a notice, `f` is swallowed but keeps it open, and the trailing Enter
    /// dismisses the notice instead of falling through to `Effect::Switch`.
    #[test]
    fn d_f_enter_on_main_never_switches() {
        let mut app = app_with(vec![
            info("main", true),
            with_status(info("feat", false), true, false),
        ]);
        // Cursor starts on main (index 0).
        let fx = app.update(key(KeyCode::Char('d')));
        assert!(fx.is_empty());
        assert!(matches!(app.overlay, Overlay::Notice { .. }));

        // `f` is absorbed and keeps the notice up.
        let fx = app.update(key(KeyCode::Char('f')));
        assert!(fx.is_empty());
        assert!(matches!(app.overlay, Overlay::Notice { .. }));

        // The trailing Enter dismisses the notice — never a Switch/Quit.
        let fx = app.update(key(KeyCode::Enter));
        assert!(
            fx.is_empty(),
            "Enter must dismiss the notice, not switch/quit: {fx:?}"
        );
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn remove_modal_cancel_produces_no_effect() {
        let mut app = three_row_app();
        app.update(key(KeyCode::Char('j')));
        app.update(key(KeyCode::Char('d')));
        let fx = app.update(key(KeyCode::Esc));
        assert!(fx.is_empty());
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn prune_uses_marked_rows_and_confirms() {
        let mut app = app_with(vec![
            info("main", true),
            with_status(info("feat-a", false), false, false),
            with_status(info("done", false), false, true),
        ]);
        // Mark feat-a (not merged, would never be auto-selected).
        app.update(key(KeyCode::Char('j')));
        app.update(key(KeyCode::Char(' ')));
        app.update(key(KeyCode::Char('p')));
        let Overlay::ConfirmPrune { candidates, .. } = &app.overlay else {
            panic!("expected ConfirmPrune");
        };
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].info.name, "feat-a");
        assert!(!candidates[0].delete_branch);

        let fx = app.update(key(KeyCode::Enter));
        match &fx[0] {
            Effect::Prune { candidates, force } => {
                assert_eq!(candidates.len(), 1);
                assert!(!force);
            }
            other => panic!("expected Prune, got {other:?}"),
        }
    }

    #[test]
    fn prune_without_marks_uses_shared_selection() {
        let mut app = app_with(vec![
            info("main", true),
            with_status(info("done", false), false, true),
            info("feat", false),
        ]);
        app.update(key(KeyCode::Char('p')));
        let Overlay::ConfirmPrune { candidates, .. } = &app.overlay else {
            panic!("expected ConfirmPrune");
        };
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].info.name, "done");
        assert!(candidates[0].reasons.contains(&"merged"));
        assert!(candidates[0].delete_branch);

        // Cancel produces no effect and closes the modal.
        let fx = app.update(key(KeyCode::Char('n')));
        assert!(fx.is_empty());
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn prune_with_nothing_to_do_shows_message() {
        let mut app = app_with(vec![info("main", true), info("feat", false)]);
        app.update(key(KeyCode::Char('p')));
        assert!(matches!(app.overlay, Overlay::None));
        assert_eq!(app.message.as_ref().unwrap().text, "nothing to prune");
    }

    #[test]
    fn create_form_prefills_base_and_submits() {
        let mut app = three_row_app();
        app.update(key(KeyCode::Char('n')));
        let Overlay::Create { base, field, .. } = &app.overlay else {
            panic!("expected Create overlay");
        };
        assert_eq!(base, "origin/main");
        assert_eq!(*field, CreateField::Branch);

        for c in "fix".chars() {
            app.update(key(KeyCode::Char(c)));
        }
        // Enter with a branch name submits with the prefilled base.
        let fx = app.update(key(KeyCode::Enter));
        match &fx[..] {
            [Effect::Create { branch, base }] => {
                assert_eq!(branch, "fix");
                assert_eq!(base, "origin/main");
            }
            other => panic!("expected Create, got {other:?}"),
        }
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn create_form_empty_base_means_head_and_empty_branch_refused() {
        let mut app = three_row_app();
        app.update(key(KeyCode::Char('n')));
        // Submitting without a branch is refused.
        let fx = app.update(key(KeyCode::Enter));
        assert!(fx.is_empty());
        assert!(app.message.as_ref().unwrap().error);

        app.update(key(KeyCode::Char('f')));
        // Tab to base, clear it entirely.
        app.update(key(KeyCode::Tab));
        for _ in 0.."origin/main".len() {
            app.update(key(KeyCode::Backspace));
        }
        let fx = app.update(key(KeyCode::Enter));
        match &fx[..] {
            [Effect::Create { branch, base }] => {
                assert_eq!(branch, "f");
                assert_eq!(base, "HEAD");
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    #[test]
    fn command_prompt_runs_in_selected_worktree() {
        let mut app = three_row_app();
        app.update(key(KeyCode::Char('j')));
        app.update(key(KeyCode::Char('x')));
        for c in "ls".chars() {
            app.update(key(KeyCode::Char(c)));
        }
        let fx = app.update(key(KeyCode::Enter));
        match &fx[..] {
            [Effect::RunCommand { path, command }] => {
                assert_eq!(path, Path::new("/wt/feat-a"));
                assert_eq!(command, "ls");
            }
            other => panic!("expected RunCommand, got {other:?}"),
        }
    }

    #[test]
    fn refresh_and_outcome_reload_rows() {
        let mut app = three_row_app();
        let fx = app.update(key(KeyCode::Char('r')));
        assert!(matches!(
            &fx[..],
            [Effect::LoadRows {
                with_status: true,
                ..
            }]
        ));
        assert!(app.status_loading);

        let fx = app.update(Msg::ActionOutcome {
            text: "done".to_string(),
            error: false,
            refresh: true,
        });
        assert!(matches!(
            &fx[..],
            [Effect::LoadRows {
                with_status: true,
                ..
            }]
        ));
        assert_eq!(app.message.as_ref().unwrap().text, "done");
    }

    #[test]
    fn stale_row_results_are_ignored_and_current_failure_settles_loading() {
        let mut app = three_row_app();
        let first = app.request_rows(true);
        let Effect::LoadRows {
            generation: old, ..
        } = first
        else {
            unreachable!()
        };
        let second = app.request_rows(true);
        let Effect::LoadRows {
            generation: current,
            ..
        } = second
        else {
            unreachable!()
        };

        app.update(Msg::RowsLoaded {
            generation: old,
            rows: vec![info("stale", false)],
            with_status: true,
        });
        assert_eq!(app.rows[0].name, "main");
        assert!(app.status_loading);

        app.update(Msg::RowsFailed {
            generation: current,
            with_status: true,
            text: "list failed: boom".to_string(),
        });
        assert!(!app.status_loading);
        assert_eq!(app.rows[0].name, "main");
        assert!(app.message.as_ref().unwrap().error);
    }

    #[test]
    fn stale_detail_result_is_ignored() {
        let mut app = three_row_app();
        let effects = app.update(key(KeyCode::Char('j')));
        let Effect::LoadDetails {
            generation: old,
            path,
        } = &effects[0]
        else {
            unreachable!()
        };
        let old = *old;
        let path = path.clone();
        app.detail_generations.insert(path.clone(), old + 1);
        app.update(Msg::Details {
            generation: old,
            path: path.clone(),
            details: None,
        });
        assert!(!app.details.contains_key(&path));
    }

    #[test]
    fn prune_waits_for_status_and_requires_force_for_dirty_candidates() {
        let mut app = app_with(vec![
            info("main", true),
            with_status(info("dirty", false), true, true),
        ]);
        app.status_loading = true;
        assert!(app.update(key(KeyCode::Char('p'))).is_empty());
        assert!(app.message.as_ref().unwrap().text.contains("status"));

        app.status_loading = false;
        app.update(key(KeyCode::Char('p')));
        let Overlay::ConfirmPrune {
            unsafe_count,
            force,
            ..
        } = &app.overlay
        else {
            panic!("expected prune confirmation");
        };
        assert_eq!(*unsafe_count, 1);
        assert!(!force);
        assert!(app.update(key(KeyCode::Enter)).is_empty());
        app.update(key(KeyCode::Char('f')));
        let effects = app.update(key(KeyCode::Enter));
        assert!(matches!(&effects[..], [Effect::Prune { force: true, .. }]));
    }

    #[test]
    fn missing_worktree_actions_are_refused_with_message() {
        let mut missing = info("gone", false);
        missing.is_missing = true;
        let mut app = app_with(vec![info("main", true), missing]);
        app.update(key(KeyCode::Char('j')));
        for code in [KeyCode::Enter, KeyCode::Char('o'), KeyCode::Char('y')] {
            let fx = app.update(key(code));
            assert!(fx.is_empty());
            assert!(app.message.as_ref().unwrap().text.contains("missing"));
        }
    }
}

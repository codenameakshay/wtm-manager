//! The root view: a title bar that clears the traffic lights, a repository
//! sidebar, and the worktree list.
//!
//! This view owns no git logic. It holds UI state, dispatches work to
//! [`crate::data`] on the background executor, and applies results back on the
//! foreground — the same model/effect split the TUI uses.
//!
//! It also hosts the three modal dialogs from [`crate::dialogs`]: at most one
//! is ever open (`self.dialog`), rendered above everything else via
//! `gpui::deferred` so it is never clipped by the list beneath it. Dialogs
//! themselves hold their pure state and presentational pieces in
//! `crate::dialogs`; the interactive wiring (click handlers, background
//! spawns, key guards) lives here, the same split [`crate::worktree_list`]
//! uses for the main list.
//!
//! Four more pieces are wired in the same spirit — pure state/rendering
//! elsewhere, interactive glue here:
//! - [`crate::watcher::RepoWatcher`] keeps `self.rows` in sync with
//!   filesystem changes outside the app (see [`WtmApp::sync_watcher`]).
//! - [`crate::detail_panel`] renders whatever `self.details` holds for the
//!   selected row; loading it is [`WtmApp::load_details_for_selection`].
//! - [`crate::context_menu`] backs right-click menus on worktree and
//!   repository rows, both funneling through one `ContextMenu<MenuTarget>`.
//! - [`crate::settings`] renders the settings sheet as a fourth overlay,
//!   mutually exclusive with `self.dialog` the same way the context menu and
//!   the dialogs already are with each other.
//! - [`crate::prefs`] is loaded once in `main.rs` and handed in; `self.prefs`
//!   is the live copy this view mutates and persists on the "meaningful
//!   change" triggers named on each setter below.
//!
//! ## Module layout
//!
//! This file is the table of contents: the `WtmApp` struct, its
//! constructor, and the `Render`/`Focusable` impls that assemble a frame
//! out of the pieces below. Everything else lives in a sibling module:
//! - `loading` — repository activation, the two-pass reload, the
//!   filesystem watcher, and detail-panel data loading.
//! - `selection` — single- and multi-row selection and the type-to-filter
//!   field.
//! - `commands` — the simpler action handlers: open/copy/terminal/reveal,
//!   context menus, the settings sheet, and preference persistence.
//! - `dialog_actions` — the Create/Remove/Prune dialogs' lifecycle and
//!   background operations, the command palette, and bulk remove.
//! - `dialog_forms` — the Create/Remove/Prune/bulk-remove dialogs' actual
//!   rendering (the "Dialog rendering" section the monolithic file used to
//!   keep separate from those dialogs' logic; that separation is now a
//!   file boundary instead of a banner comment).
//! - `chrome` — the sidebar, title bar, worktree list, and footer.
//!
//! A few small, genuinely cross-cutting pieces stay here rather than in any
//! one submodule: `MenuTarget`, `StatusMessage`, and `BulkRemoveState` are
//! named in this struct's own fields; `set_status` and `overlay_open` are
//! called from nearly every module below; and `render_modal_backdrop` is
//! shared by both dialog-rendering call sites.

mod chrome;
mod commands;
mod dialog_actions;
mod dialog_forms;
mod loading;
mod selection;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    actions, deferred, div, px, uniform_list, AnyElement, App, ClickEvent, Context, Div, Entity,
    FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Point, SharedString,
    Stateful, Subscription, Timer, Window, WindowAppearance,
};
use wtm::commands::prune::{PruneCandidate, PruneReport};
use wtm::model::WorktreeInfo;
use wtm::registry::{self, RepoEntry};
use wtm::setup::SetupEvent;
use wtm::worktree::WorktreeDetails;

use crate::assets::icons;
use crate::context_menu::{ContextMenu, MenuItem};
use crate::data::{self, OpenRepo};
use crate::detail_panel::{self, DetailTab};
use crate::dialogs::{
    self, CreatePhase, CreateState, Dialog, ProgressState, PruneState, RemoveState, StreamMsg,
};
use crate::diff_view::{self, ChangesState};
use crate::file_browser::{self, FileBrowserState, SelectedFileDiff};
use crate::palette::{self, PaletteState};
use crate::prefs::{self, Appearance, Prefs};
use crate::settings;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::{self, Theme};
use crate::ui::{self, ButtonVariant};
use crate::watcher::RepoWatcher;
use crate::worktree_list;

actions!(
    wtm,
    [
        /// Reload the active repository's worktrees.
        Reload,
        /// Open the selected worktree in the configured editor.
        OpenSelected,
        /// Move the selection down one row.
        SelectNext,
        /// Move the selection up one row.
        SelectPrev,
        /// Show or hide the repository sidebar.
        ToggleSidebar,
        /// Open the create-worktree dialog.
        NewWorktree,
        /// Open the remove-worktree confirmation for the selected row.
        RemoveSelected,
        /// Open the prune dialog.
        PruneRepo,
        /// Copy the selected worktree's path to the clipboard.
        CopyPath,
        /// Open the selected worktree in a terminal.
        OpenInTerminal,
        /// Reveal the selected worktree in Finder.
        RevealInFinder,
        /// Close whichever dialog is open; does nothing otherwise — Escape
        /// must never fall through to a destructive action.
        CloseDialog,
        /// Show or hide the detail panel for the selected worktree.
        ToggleDetailPanel,
        /// Open the settings sheet.
        OpenSettings,
        /// Open the command palette.
        OpenPalette,
        /// Focus the worktree-list filter field.
        FocusFilter,
        /// Open a native folder picker and add the chosen repository to the
        /// sidebar — the mouse-driven equivalent of running `wtm` inside it
        /// from a terminal.
        AddRepository,
        /// Show the detail panel's Details tab.
        ShowDetailsTab,
        /// Show the detail panel's Files tab (the worktree file browser).
        ShowFilesTab,
        /// Show the detail panel's Changes tab (every uncommitted diff).
        ShowChangesTab,
    ]
);

/// What a right-click context menu was opened for — a worktree row or a
/// sidebar repository row. One `ContextMenu` instance serves both, since at
/// most one may be open at a time, the same reasoning [`Dialog`] uses to be
/// an enum rather than three independent `Option`s.
#[derive(Clone)]
enum MenuTarget {
    Worktree(PathBuf),
    Repo(PathBuf),
    /// Right-clicked the list's own background rather than a row — see
    /// `commands::open_empty_space_context_menu`.
    EmptySpace,
}

/// A transient message shown in the status line.
struct StatusMessage {
    text: String,
    error: bool,
}

/// State for the bulk-remove confirmation shown when Remove is invoked
/// while more than one row is selected. [`Dialog`] (see [`crate::dialogs`])
/// cannot express "remove N arbitrary rows" — its `Remove` variant holds
/// exactly one [`WorktreeInfo`], and `dialogs.rs` is not owned by this
/// task — so this lives here instead, mutually exclusive with `self.dialog`
/// the same way `settings_open` and the palette are, and reusing
/// `dialogs::render_candidate_row`/`dialogs::render_toggle` and
/// `ui::modal_*` for its actual rendering rather than reinventing them.
///
/// `candidates` comes from `data::selection_candidates`, which already
/// applies the same safety filter `wtm prune`'s candidate selection does —
/// never the main worktree, never a protected branch — so everything in
/// this list is something `data::run_prune` (the same executor the Prune
/// dialog already uses) is actually willing to touch.
struct BulkRemoveState {
    candidates: Vec<PruneCandidate>,
    force: bool,
    busy: bool,
    error: Option<String>,
}

pub struct WtmApp {
    /// Repositories in the sidebar, most recently opened first.
    repos: Vec<RepoEntry>,
    /// The repository currently shown, if any.
    active: Option<OpenRepo>,
    /// Worktrees of the active repository, in listing order (main first).
    rows: Vec<WorktreeInfo>,
    /// Index into `rows`, kept in range whenever rows change.
    selected: Option<usize>,
    /// True until a listing carrying status fields has arrived, so the status
    /// pills can show "unknown" instead of implying "clean".
    awaiting_status: bool,
    status: Option<StatusMessage>,
    sidebar_visible: bool,
    /// Bumped on every load so a slow response for a repository the user has
    /// already navigated away from is discarded instead of overwriting the
    /// current listing.
    generation: u64,
    loading: bool,
    focus_handle: FocusHandle,
    /// The one modal dialog that may be open at a time — see
    /// [`crate::dialogs::Dialog`].
    dialog: Option<Dialog>,
    /// Branch name to select once the next listing lands. Set right after a
    /// create succeeds so the worktree the user just made is what ends up
    /// selected, rather than whatever was selected beforehand.
    pending_select: Option<String>,
    /// Live filesystem watch on the active repository, retargeted by
    /// [`WtmApp::sync_watcher`] whenever the repository or its worktree set
    /// changes. `None` both before a repository is open and when watching
    /// failed to start — either way, manual ⌘R keeps working.
    watcher: Option<RepoWatcher>,
    /// The `(git_dir, sorted worktree paths)` the watcher above is currently
    /// targeting, so `sync_watcher` can tell "nothing changed" from "retarget
    /// needed" without tearing down and recreating OS watch descriptors on
    /// every reload.
    watched: Option<(PathBuf, Vec<PathBuf>)>,
    detail_panel_visible: bool,
    /// Detail data for the selected worktree, loaded in the background by
    /// [`WtmApp::load_details_for_selection`]. `None` while loading or when
    /// nothing is selected.
    details: Option<WorktreeDetails>,
    /// The path `details` belongs to (or is currently loading for), so a
    /// stale load for a previously selected row can be told apart from the
    /// current one — mirrors `generation`, but as a separate counter,
    /// because a manual reload and a selection change are independent
    /// events that must not invalidate each other's in-flight work.
    details_path: Option<PathBuf>,
    details_generation: u64,
    /// Which section of the detail panel is showing — see
    /// [`detail_panel::DetailTab`].
    detail_tab: DetailTab,
    /// Per-worktree file-browser state (expansion, loaded directory
    /// listings, selected file), keyed by worktree path so switching the
    /// selected worktree away and back leaves the tree exactly as the user
    /// left it — see [`file_browser::FileBrowserState`].
    file_trees: HashMap<PathBuf, FileBrowserState>,
    /// Diff for whichever file is selected in the Files tab tree.
    /// [`WtmApp::load_panel_data`]/[`WtmApp::select_tree_file`] load it in
    /// the background, guarded by `details_generation` and
    /// `selected_file_diff_key` together — see those methods.
    selected_file_diff: SelectedFileDiff,
    /// The `(worktree path, rel file path)` `selected_file_diff` belongs to
    /// (or is loading for). Selecting a different file never changes
    /// `details_generation` (only a worktree-selection change does), so
    /// this is what lets a slow diff load for a file the user has since
    /// clicked away from be told apart from the current one.
    selected_file_diff_key: Option<(PathBuf, PathBuf)>,
    /// Every changed file's diff for the selected worktree — the Changes
    /// tab. Loaded the same way `details` is; see `details_generation`.
    changes: ChangesState,
    /// The worktree path `changes` belongs to (or is loading for) — mirrors
    /// `details_path`.
    changes_path: Option<PathBuf>,
    /// Right-click menu shared by worktree rows and sidebar repository rows
    /// — see [`MenuTarget`].
    context_menu: ContextMenu<MenuTarget>,
    /// What `context_menu` was opened for. Kept separately from
    /// `context_menu.target()` because the menu clears its own state
    /// *before* invoking `on_select` (see `crate::context_menu`'s dismissal
    /// convention), so `target()` would already be `None` by the time a
    /// selection needs to be resolved.
    context_menu_target: Option<MenuTarget>,
    /// The settings sheet, mutually exclusive with `dialog` — see
    /// [`WtmApp::on_open_settings`].
    settings_open: bool,
    /// Live GUI preferences, initialized from `prefs::load()` in `main.rs`
    /// and persisted by `save_prefs` on every meaningful change.
    prefs: Prefs,
    /// Type-to-filter field shown in the list header (⌘F focuses it,
    /// Escape while it has focus clears it — see its `Changed`/`Cancel`
    /// subscription wired in `new`). Always present rather than
    /// constructed on demand — unlike the dialogs and the palette, which
    /// are one-shot overlays, this needs to persist across renders the way
    /// `self.rows` does.
    filter_input: Entity<TextInput>,
    _filter_sub: Subscription,
    /// Rows selected in addition to `selected` when a shift/cmd-click has
    /// built a multi-selection. Empty means "just `selected`" — the
    /// ordinary single-selection case every existing single-target action
    /// (detail panel, ⌘⌫ on one row, Open in Editor) already assumes.
    /// Never holds exactly one element; see `apply_selection_set`.
    multi_selected: BTreeSet<usize>,
    /// The command palette overlay (⌘K), mutually exclusive with
    /// `dialog`/`settings_open`/`bulk_remove` — see `overlay_open`.
    palette: Option<PaletteState>,
    /// The bulk-remove confirmation overlay, mutually exclusive with the
    /// above the same way — see [`BulkRemoveState`].
    bulk_remove: Option<BulkRemoveState>,
}

impl WtmApp {
    pub fn new(
        initial: Option<OpenRepo>,
        prefs: Prefs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter_input = cx.new(|cx| TextInput::new("Filter", window, cx));
        // Only ever calls back through `WtmApp`'s own methods — same
        // discipline `dialogs::CreateState::new` and `palette::PaletteState::new`
        // follow; see the latter's comment for why.
        let filter_sub = cx.subscribe_in(&filter_input, window, {
            move |app: &mut WtmApp, _input, event, window, cx| match event {
                InputEvent::Changed => app.clamp_selection_to_filter(cx),
                InputEvent::Cancel => app.clear_filter(window, cx),
                InputEvent::Submit => app.on_open_selected(&OpenSelected, window, cx),
            }
        });

        let mut this = Self {
            repos: registry::load().entries(),
            active: None,
            rows: Vec::new(),
            selected: None,
            awaiting_status: true,
            status: None,
            sidebar_visible: prefs.sidebar_visible,
            generation: 0,
            loading: false,
            focus_handle: cx.focus_handle(),
            dialog: None,
            pending_select: None,
            watcher: None,
            watched: None,
            detail_panel_visible: prefs.detail_panel_visible,
            details: None,
            details_path: None,
            details_generation: 0,
            detail_tab: DetailTab::default(),
            file_trees: HashMap::new(),
            selected_file_diff: SelectedFileDiff::Unselected,
            selected_file_diff_key: None,
            changes: ChangesState::Loading,
            changes_path: None,
            context_menu: ContextMenu::new(),
            context_menu_target: None,
            settings_open: false,
            prefs,
            filter_input,
            _filter_sub: filter_sub,
            multi_selected: BTreeSet::new(),
            palette: None,
            bulk_remove: None,
        };

        if let Some(repo) = initial {
            // A clone survives `begin_activate_repo` moving `repo` into
            // `self.active` — see `seed_initial_rows` for why the startup
            // path needs a listing call of its own rather than just calling
            // `reload` like every later activation does.
            let repo_for_listing = repo.clone();
            this.begin_activate_repo(repo, cx);
            this.seed_initial_rows(repo_for_listing, cx);
        }

        this
    }

    fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = Some(StatusMessage {
            text: text.into(),
            error,
        });
    }

    /// Whether any modal overlay currently owns the window: a dialog, the
    /// settings sheet, the context menu, the command palette, or the
    /// bulk-remove confirmation. List-navigation and dialog-opening
    /// actions all guard on this so a keystroke never double-acts on both
    /// the overlay and whatever is behind it.
    ///
    /// The context-menu check in particular is defensive: its own
    /// `on_key_down` (up/down/enter/escape) does not call
    /// `cx.stop_propagation()`, so without this guard the same keystroke
    /// could also dispatch as a `WtmApp` action (e.g. `SelectNext`) on the
    /// list beneath it. `crate::context_menu` is not owned by this task, so
    /// this is a defensive workaround rather than the real fix.
    ///
    /// The palette's own `TextInput` is exactly the "focus handle mounted
    /// on the current frame" case `WtmApp::render`'s reclaim guard exists
    /// for (see that doc comment) — it is included here for the same
    /// reason `settings_open` is: so opening it never races the reclaim
    /// check into stealing focus back before the new frame paints.
    fn overlay_open(&self) -> bool {
        self.dialog.is_some()
            || self.settings_open
            || self.context_menu.is_open()
            || self.palette.is_some()
            || self.bulk_remove.is_some()
    }
}

/// The scrim behind every dialog: click anywhere outside the card to close
/// it. Shared across all three dialogs so "click outside to dismiss" is one
/// behavior, not three copies of it.
fn render_modal_backdrop(cx: &mut Context<WtmApp>) -> Stateful<Div> {
    ui::modal_backdrop()
        .id("dialog-backdrop")
        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx)))
}

impl Focusable for WtmApp {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for WtmApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);

        // The list is the window's subject, so it holds focus by default —
        // but only when no dialog is open. This must be gated on
        // `self.dialog.is_none()`, not just on `contains_focused`:
        // `FocusHandle::contains_focused` resolves containment against
        // `window.rendered_frame` — the *previously painted* frame's
        // dispatch tree — not the tree currently being built. The instant a
        // dialog opens, `on_new_worktree` calls `window.focus(&branch_focus)`
        // and schedules this render, but `branch_focus` doesn't exist
        // anywhere in `rendered_frame` yet (that frame predates the
        // dialog). `contains_focused` therefore reports `false` on that
        // first render, and an unconditional reclaim here would immediately
        // steal focus back to the root — on the very same render that was
        // supposed to hand it to the field — before the new frame (which
        // really does contain it) is ever committed. Once stolen, nothing
        // hands it back, which is exactly the "field never gets focus, ⌘N
        // types into nothing" bug. Skipping the reclaim entirely while a
        // dialog is open sidesteps the stale-frame race: `close_dialog` and
        // `submit_create_dialog` return focus to the root explicitly
        // instead of leaning on this check to notice.
        //
        // Every later overlay this view grew — the settings sheet and the
        // context menu — extends this same guard rather than reintroducing
        // the bug: `self.settings_open` follows the dialog's pattern exactly
        // (see `close_dialog`), and the context menu manages its own focus
        // entirely internally (`crate::context_menu::ContextMenu::claim_focus`
        // / its dismissal path), so it only needs to be *excluded* here, not
        // handed anything back.
        if !self.overlay_open() && !self.focus_handle.contains_focused(window, cx) {
            window.focus(&self.focus_handle);
        }

        // At most one of the dialog / settings / palette / bulk-remove
        // overlays is ever shown, kept in lockstep by the
        // `overlay_open`-gated openers above; all four use the same
        // deferred-scrim treatment, so they share one insertion point in
        // the tree below.
        let overlay = if self.settings_open {
            Some(settings::render(
                &self.prefs,
                self.active.as_ref(),
                &theme,
                cx,
            ))
        } else if self.palette.is_some() {
            Some(self.render_palette(&theme, cx))
        } else if self.bulk_remove.is_some() {
            Some(self.render_bulk_remove_dialog(&theme, cx))
        } else {
            self.render_dialog(cx)
        };
        let on_menu_select =
            cx.listener(|this, id: &str, window, cx| this.handle_menu_select(id, window, cx));
        let context_menu = self.context_menu.render(&theme, window, cx, on_menu_select);

        div()
            .id("wtm-root")
            .key_context("WtmApp")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_reload))
            .on_action(cx.listener(Self::on_open_selected))
            .on_action(cx.listener(Self::on_select_next))
            .on_action(cx.listener(Self::on_select_prev))
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_new_worktree))
            .on_action(cx.listener(Self::on_remove_selected))
            .on_action(cx.listener(Self::on_prune_repo))
            .on_action(cx.listener(Self::on_copy_path))
            .on_action(cx.listener(Self::on_open_in_terminal))
            .on_action(cx.listener(Self::on_reveal_in_finder))
            .on_action(cx.listener(Self::on_close_dialog))
            .on_action(cx.listener(Self::on_toggle_detail_panel))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_open_palette))
            .on_action(cx.listener(Self::on_focus_filter))
            .on_action(cx.listener(Self::on_add_repository))
            .on_action(cx.listener(Self::on_show_details_tab))
            .on_action(cx.listener(Self::on_show_files_tab))
            .on_action(cx.listener(Self::on_show_changes_tab))
            .size_full()
            .flex()
            .text_color(theme.text)
            // The root itself stays unpainted so the window's blurred backing
            // shows through the sidebar, the way a native source list does.
            // Only the content column gets an opaque surface.
            .when(self.sidebar_visible, |this| {
                this.child(self.render_sidebar(cx))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(theme.canvas)
                    .child(self.render_titlebar(cx))
                    .child(self.render_list(cx))
                    .child(self.render_footer(cx)),
            )
            .when(self.show_detail_panel(), |this| {
                this.child(self.render_detail_panel(cx))
            })
            // Rendered last and via `deferred` so it paints above the list
            // and sidebar regardless of source order, and is never clipped
            // by their `overflow_hidden`/scroll containers.
            .when_some(overlay, |this, overlay| {
                this.child(deferred(overlay).with_priority(1))
            })
            .when_some(context_menu, |this, menu| this.child(menu))
    }
}

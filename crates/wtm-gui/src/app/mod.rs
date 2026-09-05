//! The root view: a title bar that clears the traffic lights, a repository
//! sidebar, and the worktree list. Owns no git logic itself — it holds UI
//! state, dispatches work to [`crate::data`] on the background executor,
//! and applies results back on the foreground.
//!
//! This file is the table of contents: the `WtmApp` struct, its
//! constructor, and the `Render`/`Focusable` impls that assemble a frame
//! out of its sibling modules, each split by concern rather than by dialog
//! or widget. `MenuTarget`, `StatusMessage`, and `BulkRemoveState` stay
//! here because they're named in this struct's own fields; `set_status`,
//! `overlay_open`, and `render_modal_backdrop` stay here because nearly
//! every submodule calls them.

mod chrome;
mod commands;
mod dialog_actions;
mod dialog_forms;
#[cfg(test)]
mod integration_tests;
mod layout;
mod loading;
mod selection;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use gpui::prelude::*;
use gpui::{
    actions, deferred, div, px, uniform_list, AnyElement, App, ClickEvent, Context, Decorations,
    Div, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Point,
    ScrollHandle, ScrollStrategy, SharedString, Stateful, Subscription, UniformListScrollHandle,
    Window, WindowAppearance,
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
use crate::motion;
use crate::palette::{self, PaletteState};
use crate::prefs::{self, Appearance, Prefs};
use crate::run_panel::{self, RunCommandState};
use crate::settings;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::{self, Theme};
use crate::ui::{self, ButtonVariant};
use crate::watcher::RepoWatcher;
use crate::window_frame;
use crate::worktree_list::{self, SortMode};

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
        /// Fetch the active repository's default remote (`git fetch
        /// --prune`), refreshing ahead/behind counts and "upstream gone"
        /// detection.
        FetchRemote,
        /// Open the "Run Command" dialog for the selected worktree.
        RunCommand,
        /// Move keyboard focus to the next Tab stop. gpui-0.2.2 ships the
        /// tab-stop machinery (`Window::focus_next`, `elements/div.rs`'s
        /// `tab_stop`/`tab_index`/`tab_group`) but binds no key to it — see
        /// `main.rs`'s `key_bindings!` entry for this action.
        FocusNext,
        /// Move keyboard focus to the previous Tab stop — the Shift-Tab
        /// counterpart to [`FocusNext`].
        FocusPrev,
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
/// with more than one row selected. `Dialog::Remove` holds exactly one
/// [`WorktreeInfo`], so this lives beside `self.dialog` (mutually exclusive
/// with it) and reuses the dialogs' row/toggle rendering.
///
/// `candidates` comes from `data::selection_candidates`, which applies the
/// same never-main, never-protected filter as `wtm prune`.
struct BulkRemoveState {
    candidates: Vec<PruneCandidate>,
    force: bool,
    busy: bool,
    /// Candidates dealt with so far while `busy`, for the "n of N" line.
    done: usize,
    error: Option<String>,
}

pub struct WtmApp {
    /// Repositories in the sidebar, alphabetical by name (see
    /// `sidebar_sorted`) — a stable order that does not reshuffle when a
    /// repo is opened, unlike `Registry::entries()`'s own
    /// most-recently-opened-first order, which is what the CLI still uses.
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
    /// How `rows` is ordered — see [`SortMode`]. Session-only: nothing
    /// persists this across a restart yet, unlike `sidebar_visible`/
    /// `detail_panel_visible`, which live in `Prefs`.
    sort_mode: SortMode,
    /// HEAD commit unix-time per worktree path, for `Recent`-mode sorting
    /// and each row's age display — loaded in the background after every
    /// listing lands (see `loading::spawn_activity_load`), guarded by
    /// `generation` the same way `rows` itself is. A worktree missing from
    /// this map (still loading, or no resolvable HEAD) shows no age rather
    /// than a guess — see `worktree_list::render_row`.
    activity: HashMap<PathBuf, i64>,
    /// A `git fetch` is currently running — the in-flight guard `FetchRemote`
    /// checks before starting another one, since a second concurrent fetch
    /// against the same repository is at best wasted work. Cleared in
    /// `apply_fetch_result` regardless of outcome.
    fetching: bool,
    sidebar_visible: bool,
    /// Bumped on every load so a slow response for a repository the user has
    /// already navigated away from is discarded instead of overwriting the
    /// current listing.
    generation: u64,
    loading: bool,
    focus_handle: FocusHandle,
    /// Where focus lands when a confirmation dialog with no text field
    /// opens (Remove, Prune, and the bulk-remove confirmation): the Cancel
    /// button in each — the safe action, never the destructive one. One
    /// shared handle rather than a field on each of `RemoveState`/
    /// `PruneState`/`BulkRemoveState`: `overlay_open` already guarantees at
    /// most one of those is ever showing, so there is never a collision,
    /// and keeping it here means `dialogs.rs`'s state constructors (and
    /// their existing plain, `cx`-free unit tests) don't need to grow a
    /// `Context<WtmApp>` parameter just to mint a `FocusHandle`.
    dialog_safe_focus: FocusHandle,
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
    /// Whether the window is currently active. Filesystem changes received
    /// while inactive only mark the repository stale; activation performs one
    /// coalesced refresh so background Git activity cannot spend CPU scanning
    /// every worktree while the app is hidden behind another window.
    window_active: bool,
    repository_stale: bool,
    /// A prune or bulk remove this app started is still running. Every
    /// worktree it deletes fires the watcher; reloading on each one would
    /// re-scan every remaining worktree per removal, so those events only
    /// set `repository_stale` and the operation's completion reloads once.
    prune_in_flight: bool,
    _activation_sub: Subscription,
    detail_panel_visible: bool,
    /// True once the user has explicitly reopened the detail panel
    /// (`commands::on_toggle_detail_panel`) while the window was too narrow
    /// for it to fit under `layout::DETAIL_PANEL_BREAKPOINT` — see
    /// `layout::detail_panel_should_show`'s doc for what this overrides and
    /// why. Session-only, never persisted to `Prefs`: it describes "the
    /// window is narrow right now and I asked for this anyway," not a
    /// standing preference, and `layout::narrow_override_after_resize`
    /// (called once per render in `Render::render`) drops it again the
    /// moment the window is wide enough that it isn't doing anything.
    detail_panel_narrow_override: bool,
    /// Detail data for the selected worktree, loaded in the background by
    /// [`WtmApp::load_details_for_selection`]. `None` while loading or when
    /// nothing is selected.
    details: Option<WorktreeDetails>,
    /// The path `details` belongs to (or is currently loading for), so a
    /// stale load for a row that has since fallen out of selection can be
    /// told apart from the current one — mirrors `generation`, but as a
    /// separate counter, because a manual reload and a selection change are
    /// independent events that must not invalidate each other's in-flight
    /// work.
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
    context_menu: ContextMenu,
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
    /// The "Run Command" dialog, mutually exclusive with the above the same
    /// way — see [`crate::run_panel::RunCommandState`]'s module doc for why
    /// this is its own field rather than a fourth `dialogs::Dialog` variant.
    run_command: Option<RunCommandState>,
    /// Commands recently run via the Run Command dialog, most-recent-first,
    /// keyed by repository (its main worktree root, `OpenRepo::path()`) so a
    /// build/test command typed in one repo doesn't clutter another's
    /// suggestions. Session-only: nothing persists this across a restart yet.
    recent_commands: HashMap<PathBuf, Vec<String>>,
    /// The worktree list's own scroll position — `ui::scrollbar`/
    /// `ui::scroll_fade_*` both need a live handle to read geometry off of,
    /// which `uniform_list` only exposes once tracked (`UniformListScrollHandle`
    /// wraps a plain `ScrollHandle` — see `UniformListScrollState::base_handle`
    /// in the vendored `gpui-0.2.2` source).
    list_scroll: UniformListScrollHandle,
    /// The Changes tab's own scroll position — same reasoning as
    /// `list_scroll`, for `render_changes_tab`'s `"changes-scroll"` region
    /// (the literal panel the user reported has no scrollbar).
    changes_scroll: ScrollHandle,
    /// The Files tab's tree column's own scroll position.
    files_tree_scroll: ScrollHandle,
    /// The Files tab's diff column's own scroll position.
    files_diff_scroll: ScrollHandle,
    /// The settings sheet's own scroll position, threaded into
    /// `settings::render` (`settings.rs` owns no persistent state of its
    /// own — see that module's doc — so this lives here like every other
    /// overlay's scroll handle).
    settings_scroll: ScrollHandle,
}

/// Sort registry entries into the order the sidebar displays them in:
/// alphabetically by name (case-insensitive), path as tie-break.
///
/// `Registry::entries()` returns most-recently-opened first, which the CLI
/// relies on; using that here would make selecting a repo (which bumps
/// `last_opened`) jump it to the top under the user's cursor. Every
/// assignment to `self.repos` routes through this.
fn sidebar_sorted(mut entries: Vec<RepoEntry>) -> Vec<RepoEntry> {
    entries.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.path.cmp(&b.path))
    });
    entries
}

impl WtmApp {
    pub fn new(
        initial: Option<OpenRepo>,
        prefs: Prefs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter_input = cx.new(|cx| TextInput::new("Filter", cx));
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
            repos: sidebar_sorted(registry::load().entries()),
            active: None,
            rows: Vec::new(),
            selected: None,
            awaiting_status: true,
            status: None,
            sort_mode: SortMode::default(),
            activity: HashMap::new(),
            fetching: false,
            sidebar_visible: prefs.sidebar_visible,
            generation: 0,
            loading: false,
            focus_handle: cx.focus_handle(),
            dialog_safe_focus: cx.focus_handle().tab_stop(true).tab_index(0),
            dialog: None,
            pending_select: None,
            watcher: None,
            watched: None,
            window_active: window.is_window_active(),
            repository_stale: false,
            prune_in_flight: false,
            _activation_sub: cx.observe_window_activation(window, |app, window, cx| {
                app.on_window_activation(window, cx)
            }),
            detail_panel_visible: prefs.detail_panel_visible,
            detail_panel_narrow_override: false,
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
            run_command: None,
            recent_commands: HashMap::new(),
            list_scroll: UniformListScrollHandle::new(),
            changes_scroll: ScrollHandle::new(),
            files_tree_scroll: ScrollHandle::new(),
            files_diff_scroll: ScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
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

    /// `set_status(text, true)` plus the `cx.notify()` every call site
    /// needs anyway — an error worth reading, never cleared by a
    /// background refresh (see `apply_rows`'s doc on that guarantee).
    fn set_error(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.set_status(text, true);
        cx.notify();
    }

    /// `set_status(text, false)` plus `cx.notify()` — a purely
    /// informational message.
    fn set_info(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.set_status(text, false);
        cx.notify();
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
    /// list beneath it — a defensive workaround here rather than a fix in
    /// `crate::context_menu` itself.
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
            || self.run_command.is_some()
    }

    /// `Theme::of(cx)` for the background shell specifically — the
    /// sidebar, title bar, worktree list, and detail panel (`chrome.rs`'s
    /// `render_sidebar`/`render_titlebar`/`render_list`/`render_footer`/
    /// `render_detail_panel`). Forces [`Theme::tab_stops`] to `false`
    /// whenever [`Self::overlay_open`], so Tab/Shift-Tab can't walk out of
    /// an open dialog into the shell painted behind it; see that field's
    /// doc for why gpui's own `tab_group()` alone can't do this. Every
    /// overlay's own render path (`render_dialog`, `settings::render`,
    /// `render_palette`, `render_bulk_remove_dialog`,
    /// `render_run_command_dialog`, `ContextMenu::render`) must keep
    /// calling `Theme::of(cx)` directly instead — routing an overlay's own
    /// content through this method would make its own controls
    /// unreachable by Tab too.
    fn chrome_theme(&self, cx: &App) -> Theme {
        let theme = Theme::of(cx);
        if self.overlay_open() {
            Theme {
                tab_stops: false,
                ..theme
            }
        } else {
            theme
        }
    }
}

/// The scrim behind every dialog: click anywhere outside the card to close
/// it. Shared across all three `dialogs::Dialog` variants, the bulk-remove
/// confirmation, and (from `crate::run_panel`, outside this module) the Run
/// Command dialog, so "click outside to dismiss" is one behavior, not
/// several copies of it. `pub(crate)`, not private, specifically so
/// `run_panel::render` — which lives outside `app` and so cannot be a
/// method on `WtmApp` the way the dialog-rendering methods are — can reuse
/// it too.
pub(crate) fn render_modal_backdrop(cx: &mut Context<WtmApp>) -> Stateful<Div> {
    ui::modal_backdrop()
        .id("dialog-backdrop")
        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx)))
    // `ui::modal_backdrop()` already carries `.occlude()` — see its doc —
    // so the worktree list behind every dialog is safe from both the
    // click and the scroll-wheel leak this function's own doc describes.
}

/// The two-layer entrance every modal card in this app uses: `DIALOG_IN` on
/// the card itself, `FADE_QUICK` on the scrim behind it. `id` names the
/// dialog (e.g. `"settings-dialog"`) and must be unique per modal — it
/// becomes `"{id}-in"`/`"{id}-backdrop-in"` for the two animations' own ids.
pub(crate) fn present_modal(
    id: &'static str,
    card: impl IntoElement + gpui::Styled + 'static,
    cx: &mut Context<WtmApp>,
) -> AnyElement {
    let backdrop = render_modal_backdrop(cx).child(motion::dialog_in(
        SharedString::from(format!("{id}-in")),
        card,
        cx,
    ));
    motion::fade_quick(
        SharedString::from(format!("{id}-backdrop-in")),
        backdrop,
        cx,
    )
    .into_any_element()
}

impl Focusable for WtmApp {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for WtmApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);

        // Width-adaptive state, resolved once per render before anything
        // below reads it — see `layout`'s module doc. Two things happen
        // here, both driven purely by the current viewport width:
        //
        // 1. The detail panel's `narrow_override` (see that field's doc)
        //    is dropped the instant the window is wide enough that it
        //    isn't overriding anything — so a *future* narrowing
        //    auto-collapses fresh rather than staying silently exempted by
        //    a click from an earlier, unrelated narrow session.
        // 2. The Files/Changes tabs' wide panel (`layout::wide_tabs_fit`)
        //    has no user override at all (see `layout::WIDE_TABS_BREAKPOINT`'s
        //    doc: below it, the list column the wide panel would leave
        //    behind isn't just tight, it can go negative) — so if the
        //    window has narrowed out from under an already-active
        //    Files/Changes tab, this snaps back to Details before the tree
        //    below ever builds, rather than painting a tab whose panel
        //    doesn't fit for one frame and then yanking it back.
        let viewport_width = f32::from(window.viewport_size().width);
        self.detail_panel_narrow_override =
            layout::narrow_override_after_resize(viewport_width, self.detail_panel_narrow_override);
        if !layout::wide_tabs_fit(viewport_width)
            && matches!(self.detail_tab, DetailTab::Files | DetailTab::Changes)
        {
            self.detail_tab = DetailTab::Details;
        }

        // The list is the window's subject, so it holds focus by default —
        // but only when no overlay is open. Gated on `overlay_open()`, not
        // just `contains_focused`: that check resolves against the
        // last-painted frame, which doesn't yet contain a focus
        // handle an overlay just opened this render, so an unconditional
        // reclaim would steal it back before the new frame (which does
        // contain it) is ever committed. `close_dialog` and
        // `submit_create_dialog` return focus to the root explicitly
        // instead of leaning on this check to notice.
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
                &self.settings_scroll,
                &theme,
                cx,
            ))
        } else if self.palette.is_some() {
            Some(self.render_palette(&theme, cx))
        } else if self.bulk_remove.is_some() {
            Some(self.render_bulk_remove_dialog(&theme, cx))
        } else if self.run_command.is_some() {
            Some(self.render_run_command_dialog(&theme, cx))
        } else {
            self.render_dialog(cx)
        };
        let on_menu_select =
            cx.listener(|this, id: &str, window, cx| this.handle_menu_select(id, window, cx));
        let context_menu = self.context_menu.render(&theme, window, cx, on_menu_select);

        let root = div()
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
            .on_action(cx.listener(Self::on_fetch_remote))
            .on_action(cx.listener(Self::on_run_command))
            .on_action(cx.listener(Self::on_focus_next))
            .on_action(cx.listener(Self::on_focus_prev))
            .size_full()
            .flex()
            .text_color(theme.text)
            // Sets the default text family for the whole window (and, via
            // gpui's text-style cascade, every deferred/anchored overlay
            // painted under it) — everything except `ui::Tooltip`, which
            // gpui prepaints as a separate root and sets this itself.
            .font_family(theme.font_sans)
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
                    .bg(theme.bg)
                    .child(self.render_titlebar(window, cx))
                    .child(self.render_list(cx))
                    .child(self.render_footer(window, cx)),
            )
            .child(self.render_detail_panel(window, cx))
            // Rendered last and via `deferred` so it paints above the list
            // and sidebar regardless of source order, and is never clipped
            // by their `overflow_hidden`/scroll containers.
            .when_some(overlay, |this, overlay| {
                this.child(deferred(overlay).with_priority(1))
            })
            .when_some(context_menu, |this, menu| this.child(menu));

        // Client-side decorations (Linux, when the compositor grants them —
        // see `window_frame`'s module doc): wraps `root` in a shadow margin,
        // rounded corners, and resize handles. A no-op everywhere else,
        // including macOS, which never reports anything but
        // `Decorations::Server`.
        window_frame::wrap(root, &theme, window)
    }
}

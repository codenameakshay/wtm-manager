//! The simpler action handlers and the small operations they invoke
//! directly: open in editor/terminal, reveal in Finder, copy path, the
//! sidebar's repository context menu and a worktree row's context menu,
//! toggling the sidebar/detail panel, the settings sheet's open action, and
//! preference persistence (`save_prefs` and friends).
//!
//! Heavier, multi-step operations that revolve around a modal dialog or an
//! overlay (Create/Remove/Prune, the command palette, bulk remove) live in
//! `dialog_actions` instead, not here — the split follows the same "how
//! big is the state machine behind this action" line the original file's
//! banner comments already drew.

use super::*;

use crate::motion;

impl WtmApp {
    pub(super) fn on_toggle_detail_panel(
        &mut self,
        _: &ToggleDetailPanel,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.detail_panel_visible = !self.detail_panel_visible;
        self.prefs.detail_panel_visible = self.detail_panel_visible;
        self.save_prefs();
        cx.notify();
    }

    // -------------------------------------------------------------
    // Preferences
    // -------------------------------------------------------------

    /// Persist `self.prefs`, surfacing a failure the same way a registry
    /// write failure is surfaced: a papercut worth telling the user about,
    /// never a reason to interrupt what they were doing.
    pub(super) fn save_prefs(&mut self) {
        if let Err(e) = prefs::save(&self.prefs) {
            self.set_status(format!("could not save preferences: {e}"), true);
        }
    }

    /// A snapshot of the live preferences for `main.rs` to augment with the
    /// window frame and persist on close, without `main.rs` needing to know
    /// anything else about this view's internals.
    pub(crate) fn prefs_snapshot(&self) -> Prefs {
        self.prefs.clone()
    }

    /// Whether the appearance preference is `System` — `main.rs`'s
    /// `observe_window_appearance` callback checks this before re-resolving
    /// the theme from the OS, so a forced Light/Dark choice survives a live
    /// system appearance change instead of being silently overridden the
    /// next time it fires.
    pub(crate) fn follows_system_appearance(&self) -> bool {
        self.prefs.appearance == Appearance::System
    }

    /// Set the appearance preference, persist it, and apply it immediately.
    /// `Light`/`Dark` are forced by calling `theme::refresh` with that exact
    /// `WindowAppearance` regardless of what the OS is actually running —
    /// `theme::refresh` only ever branches on the two-way light/dark split,
    /// so this reaches every palette its API can produce without editing
    /// `theme.rs`. `System` re-resolves from the window's real appearance.
    pub(crate) fn set_appearance(
        &mut self,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prefs.appearance = appearance;
        self.save_prefs();
        match appearance {
            Appearance::System => theme::refresh(window.appearance(), cx),
            Appearance::Light => theme::refresh(WindowAppearance::Light, cx),
            Appearance::Dark => theme::refresh(WindowAppearance::Dark, cx),
        }
        cx.notify();
    }

    /// Set the reduce-motion preference, persist it, and apply it
    /// immediately — same shape as [`Self::set_appearance`]: write
    /// `self.prefs`, persist, then push the live value to the runtime global
    /// (`motion::set_reduced`) the same render pass reads back via
    /// `motion::reduced`, so the toggle and actual animation behavior can
    /// never disagree.
    pub(crate) fn set_reduce_motion(&mut self, value: bool, cx: &mut Context<Self>) {
        self.prefs.reduce_motion = value;
        self.save_prefs();
        motion::set_reduced(cx, value);
        cx.notify();
    }

    // -------------------------------------------------------------
    // Settings sheet
    // -------------------------------------------------------------

    pub(super) fn on_open_settings(
        &mut self,
        _: &OpenSettings,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_open() {
            return;
        }
        self.settings_open = true;
        cx.notify();
    }

    // -------------------------------------------------------------
    // Fetch
    // -------------------------------------------------------------

    /// Fetch the active repository's default remote in the background: the
    /// ⌘⇧F binding, the list toolbar's Fetch button, and the empty-space
    /// context menu's "Fetch" item all funnel through this one handler.
    ///
    /// Ahead/behind counts and prune's "upstream gone" detection are only
    /// ever as fresh as the last fetch (see `data::fetch`'s own doc) — that
    /// is why a successful fetch reloads the listing immediately in
    /// `apply_fetch_result`, rather than leaving the user to notice the
    /// pills are stale.
    pub(super) fn on_fetch_remote(
        &mut self,
        _: &FetchRemote,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_open() || self.fetching {
            // `self.fetching` is the real guard: every trigger for this
            // action funnels through this one method, so checking it here
            // — not just disabling the toolbar button's appearance — is
            // what actually makes a second concurrent `git fetch` against
            // the same repository impossible rather than merely
            // discouraged.
            return;
        }
        let Some(repo) = self.active.clone() else {
            return;
        };

        self.fetching = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move { data::fetch(&repo, None) })
                .await;
            this.update(cx, |this, cx| this.apply_fetch_result(outcome, cx))
                .ok();
        })
        .detach();
    }

    /// Report a finished fetch and, on success, reload the listing.
    ///
    /// On failure the message stays: `set_status(.., true)` is exactly what
    /// `apply_rows` promises never to clear on its own (see that method's
    /// doc comment) — a failed fetch never reloads on this path, but the
    /// same guarantee also protects the error from being wiped by anything
    /// else that reloads afterward (a manual ⌘R, the filesystem watcher).
    fn apply_fetch_result(
        &mut self,
        result: Result<data::FetchOutcome, String>,
        cx: &mut Context<Self>,
    ) {
        self.fetching = false;
        match result {
            Ok(outcome) => {
                let message = if outcome.updated_refs > 0 {
                    format!(
                        "fetched {} · {} ref{} updated",
                        outcome.remote,
                        outcome.updated_refs,
                        if outcome.updated_refs == 1 { "" } else { "s" }
                    )
                } else {
                    format!("fetched {} · already up to date", outcome.remote)
                };
                self.set_status(message, false);
                self.reload(cx);
            }
            Err(e) => self.set_status(format!("fetch failed: {e}"), true),
        }
        cx.notify();
    }

    // -------------------------------------------------------------
    // Context menus
    // -------------------------------------------------------------

    pub(super) fn open_worktree_context_menu(
        &mut self,
        row_ix: usize,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(info) = self.rows.get(row_ix).cloned() else {
            return;
        };
        // Right-clicking a row also selects it, so the menu and the visible
        // selection never disagree — but only when no multi-selection is
        // already active. Unconditionally collapsing one here (the old
        // behavior) would make "Add to Selection" below a lie: there would
        // be nothing left to add to by the time the menu opens. A row
        // outside the current multi-selection is still just described by
        // the menu, not folded into it — that is what the "Add to
        // Selection" item itself is for.
        if self.multi_selected.is_empty() {
            self.select(row_ix, cx);
        }

        // The discoverable, click-target equivalent of a shift/⌘-click,
        // named for whichever of the three states actually applies right
        // now rather than a generic "Toggle Selection" a user would have to
        // click once just to find out what it does.
        let select_item = if self.multi_selected.contains(&row_ix) {
            MenuItem::action("toggle-select", "Remove from Selection").icon(icons::CLOSE)
        } else if !self.multi_selected.is_empty() {
            MenuItem::action("toggle-select", "Add to Selection")
                .icon(icons::CHECK)
                .shortcut("⌘-click")
        } else {
            MenuItem::action("toggle-select", "Select")
                .icon(icons::CHECK)
                .shortcut("⌘-click")
        };

        let remove_item = if info.is_main {
            MenuItem::action("remove", "Remove…")
                .icon(icons::TRASH)
                .danger()
                .disabled()
                .shortcut("main worktree")
        } else {
            MenuItem::action("remove", "Remove…")
                .icon(icons::TRASH)
                .danger()
                .shortcut("⌘⌫")
        };
        let items = vec![
            MenuItem::action("open-editor", "Open in Editor")
                .icon(icons::OPEN_EXTERNAL)
                .shortcut("⏎"),
            MenuItem::action("run-command", "Run Command…").shortcut("⌘E"),
            MenuItem::action("open-terminal", "Open in Terminal").shortcut("⌘⇧T"),
            self.open_remote_menu_item(&info),
            MenuItem::action("reveal-finder", "Reveal in Finder").shortcut("⌘⇧R"),
            MenuItem::action("copy-path", "Copy Path")
                .icon(icons::COPY)
                .shortcut("⌘C"),
            MenuItem::separator(),
            select_item,
            MenuItem::separator(),
            remove_item,
        ];

        let target = MenuTarget::Worktree(info.path);
        self.context_menu_target = Some(target.clone());
        self.context_menu.open(target, position, items);
        cx.notify();
    }

    /// Right-clicked the list's own background rather than a row — the
    /// standard place users look for "do something here", and (per the
    /// user's own complaint that prompted this task) previously did nothing
    /// at all. `New Worktree`/`Prune…`/`Reload` need an open repository;
    /// shown but disabled (with the reason in the shortcut slot, same idiom
    /// as the worktree row menu's main-worktree `Remove…`) rather than
    /// hidden, so right-clicking an empty window never looks broken.
    pub(super) fn open_empty_space_context_menu(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_open() {
            return;
        }
        let has_repo = self.active.is_some();
        // Not just `repo_scoped_item(has_repo, ..)`: Fetch has a second way
        // to be unavailable (already running) that none of the other
        // repo-scoped items do, so it needs its own disabled-reason text
        // rather than that helper's single `has_repo` check.
        let fetch_item = if !has_repo {
            MenuItem::action("fetch", "Fetch")
                .icon(icons::REFRESH)
                .disabled()
                .shortcut("open a repository first")
        } else if self.fetching {
            MenuItem::action("fetch", "Fetch")
                .icon(icons::REFRESH)
                .disabled()
                .shortcut("fetching…")
        } else {
            MenuItem::action("fetch", "Fetch")
                .icon(icons::REFRESH)
                .shortcut("⌘⇧F")
        };
        let items = vec![
            repo_scoped_item(has_repo, "new-worktree", "New Worktree", icons::PLUS, "⌘N"),
            fetch_item,
            repo_scoped_item(has_repo, "prune", "Prune…", icons::TRASH, "⌘⇧P"),
            repo_scoped_item(has_repo, "reload", "Reload", icons::REFRESH, "⌘R"),
            MenuItem::separator(),
            MenuItem::action("add-repository", "Add Repository…")
                .icon(icons::PLUS)
                .shortcut("⌘⇧O"),
        ];

        let target = MenuTarget::EmptySpace;
        self.context_menu_target = Some(target.clone());
        self.context_menu.open(target, position, items);
        cx.notify();
    }

    pub(super) fn open_repo_context_menu(
        &mut self,
        path: PathBuf,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        // Right-clicking a repo row also opens it, for the same reason a
        // worktree row's right-click also selects it: the menu that appears
        // must describe whatever is now on screen, not whatever was on
        // screen a moment ago.
        self.select_repo(path.clone(), cx);

        let items = vec![
            MenuItem::action("open", "Open").icon(icons::FOLDER),
            MenuItem::action("reveal-finder", "Reveal in Finder"),
            MenuItem::action("copy-path", "Copy Path").icon(icons::COPY),
            MenuItem::separator(),
            // "from Sidebar" is deliberate: this only forgets the registry
            // entry (see `forget_repo`) and must never read as "delete the
            // repository".
            MenuItem::action("forget", "Remove from Sidebar")
                .icon(icons::TRASH)
                .danger(),
        ];

        let target = MenuTarget::Repo(path);
        self.context_menu_target = Some(target.clone());
        self.context_menu.open(target, position, items);
        cx.notify();
    }

    pub(super) fn handle_menu_select(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.context_menu_target.take() else {
            return;
        };
        match target {
            MenuTarget::Worktree(path) => self.handle_worktree_menu_action(&path, id, window, cx),
            MenuTarget::Repo(path) => self.handle_repo_menu_action(&path, id, cx),
            MenuTarget::EmptySpace => self.handle_empty_space_menu_action(id, window, cx),
        }
    }

    fn handle_worktree_menu_action(
        &mut self,
        path: &Path,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match id {
            "open-editor" => self.open_path_in_editor(path.to_path_buf(), cx),
            "run-command" => {
                if let Some(info) = self.rows.iter().find(|row| row.path == path).cloned() {
                    self.open_run_command_dialog(info, window, cx);
                }
            }
            "open-terminal" => self.open_in_terminal_path(path.to_path_buf(), cx),
            "open-remote" => {
                if let Some(info) = self.rows.iter().find(|row| row.path == path).cloned() {
                    self.open_remote_for(info, cx);
                }
            }
            "reveal-finder" => self.reveal_path_in_finder(path.to_path_buf(), cx),
            "copy-path" => self.copy_path_to_clipboard(path.to_path_buf(), cx),
            "toggle-select" => {
                if let Some(row_ix) = self.rows.iter().position(|row| row.path == path) {
                    self.toggle_row_selection(row_ix, cx);
                }
            }
            "remove" => {
                if let Some(info) = self.rows.iter().find(|row| row.path == path).cloned() {
                    self.open_remove_dialog_for(info, cx);
                }
            }
            _ => {}
        }
    }

    /// Dispatch an empty-space menu choice through the exact same `on_*`
    /// method its real keystroke or toolbar button already calls — same
    /// "no new behavior, only another way to reach it" discipline
    /// `dialog_actions::run_palette_command` already follows for the
    /// palette.
    fn handle_empty_space_menu_action(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match id {
            "new-worktree" => self.on_new_worktree(&NewWorktree, window, cx),
            "fetch" => self.on_fetch_remote(&FetchRemote, window, cx),
            "prune" => self.on_prune_repo(&PruneRepo, window, cx),
            "reload" => self.on_reload(&Reload, window, cx),
            "add-repository" => self.on_add_repository(&AddRepository, window, cx),
            _ => {}
        }
    }

    fn handle_repo_menu_action(&mut self, path: &Path, id: &str, cx: &mut Context<Self>) {
        match id {
            "open" => self.select_repo(path.to_path_buf(), cx),
            "reveal-finder" => self.reveal_path_in_finder(path.to_path_buf(), cx),
            "copy-path" => self.copy_path_to_clipboard(path.to_path_buf(), cx),
            "forget" => self.forget_repo(path, cx),
            _ => {}
        }
    }

    /// Drop `path` from the sidebar registry. Never touches the filesystem —
    /// see `wtm::registry::Registry::forget`'s own doc on that guarantee.
    fn forget_repo(&mut self, path: &Path, cx: &mut Context<Self>) {
        let mut reg = registry::load();
        if reg.forget(path) {
            match registry::save(&reg) {
                Ok(()) => {
                    self.repos = reg.entries();
                    self.set_status("removed from sidebar", false);
                }
                Err(e) => self.set_status(format!("could not save the repo list: {e}"), true),
            }
        }
        cx.notify();
    }

    // -------------------------------------------------------------
    // Open on Remote
    // -------------------------------------------------------------

    /// Build the worktree row menu's "Open on Remote…" item: enabled with
    /// its real shortcut (none — this action's availability depends on the
    /// selected worktree, so it has no fixed global keybinding) when
    /// `data::remote_branch_url` can resolve a browsable URL for this
    /// worktree's branch, disabled with the reason otherwise — never
    /// present-but-broken.
    ///
    /// Resolves the URL synchronously, directly in this (already
    /// synchronous, one-off, click-triggered) menu-building call, rather
    /// than through `cx.background_spawn`: unlike `data::list_branches`/
    /// `list_refs` (which walk every branch in the repository),
    /// `remote_branch_url` is at most two `git2` lookups plus string
    /// parsing — no loop over the ref set — and this app's context menus
    /// have no "loading…" state to show while an item's availability is
    /// still being determined. `select_repo` already makes the same
    /// "small, synchronous git read directly in a click handler" tradeoff
    /// for `data::open_repo` (full repo discovery + config parsing, more
    /// work than this), so this follows existing precedent rather than
    /// setting a new one.
    fn open_remote_menu_item(&self, info: &WorktreeInfo) -> MenuItem {
        let base = MenuItem::action("open-remote", "Open on Remote…").icon(icons::OPEN_EXTERNAL);
        let Some(repo) = self.active.as_ref() else {
            return base.disabled().shortcut("open a repository first");
        };
        let url = info
            .branch
            .as_deref()
            .and_then(|branch| data::remote_branch_url(repo, branch));
        match open_remote_disabled_reason(info.branch.is_some(), url.as_deref()) {
            Some(reason) => base.disabled().shortcut(reason),
            None => base,
        }
    }

    /// ⌘? (no binding today — see `open_remote_menu_item`'s doc comment)
    /// and the "Open on Remote…" command in the palette: open the selected
    /// worktree's branch on its remote host. A no-op with nothing selected,
    /// matching every other single-target action's guard in this file.
    pub(super) fn on_open_remote(
        &mut self,
        _: &OpenRemote,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.selected else {
            return;
        };
        let Some(info) = self.rows.get(ix).cloned() else {
            return;
        };
        self.open_remote_for(info, cx);
    }

    /// Resolve `info`'s branch to a remote URL and open it in the system
    /// browser. Shared by `on_open_remote` and the worktree row's context
    /// menu item. Unlike `open_remote_menu_item`'s synchronous check above,
    /// `data::open_url` (which forks a subprocess) runs through
    /// `cx.background_spawn`, the same as `open_in_terminal_path`/
    /// `reveal_path_in_finder` already do for their own subprocess calls.
    pub(super) fn open_remote_for(&mut self, info: WorktreeInfo, cx: &mut Context<Self>) {
        let Some(repo) = self.active.clone() else {
            return;
        };
        let Some(branch) = info.branch.clone() else {
            self.set_status(
                "this worktree has no branch (detached HEAD) — nothing to open",
                true,
            );
            cx.notify();
            return;
        };
        let Some(url) = data::remote_branch_url(&repo, &branch) else {
            self.set_status(format!("no remote is configured for '{branch}'"), true);
            cx.notify();
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { data::open_url(&url) })
                .await;
            this.update(cx, |this, cx| {
                if let Err(e) = result {
                    this.set_status(format!("could not open browser: {e}"), true);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn open_row_in_editor(&mut self, row_ix: usize, cx: &mut Context<Self>) {
        let Some(path) = self.rows.get(row_ix).map(|info| info.path.clone()) else {
            return;
        };
        self.open_path_in_editor(path, cx);
    }

    /// Open `path` in the configured editor. Shared by "open selected row"
    /// and the create dialog's "Open in Editor" button — the latter can't
    /// go through `open_row_in_editor` because the new worktree isn't in
    /// `self.rows` yet the moment creation finishes; `reload` hasn't landed.
    pub(super) fn open_path_in_editor(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(repo) = self.active.clone() else {
            return;
        };

        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn({
                    let path = path.clone();
                    async move { data::open_in_editor(&repo, &path) }
                })
                .await;
            this.update(cx, |this, cx| {
                match outcome {
                    Ok(()) => this.set_status(format!("opened {}", path.display()), false),
                    Err(e) => this.set_status(format!("open failed: {e}"), true),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn on_reload(&mut self, _: &Reload, _window: &mut Window, cx: &mut Context<Self>) {
        self.reload(cx);
    }

    pub(super) fn on_open_selected(
        &mut self,
        _: &OpenSelected,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A dialog's own Enter binding (submit the form, or nothing for
        // Remove/Prune) takes priority; the list beneath it must not also
        // react to the same keystroke.
        if self.overlay_open() {
            return;
        }
        if let Some(row_ix) = self.selected {
            self.open_row_in_editor(row_ix, cx);
        }
    }

    pub(super) fn on_toggle_sidebar(
        &mut self,
        _: &ToggleSidebar,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_visible = !self.sidebar_visible;
        self.prefs.sidebar_visible = self.sidebar_visible;
        self.save_prefs();
        cx.notify();
    }

    /// Open a repository chosen from the sidebar.
    pub(super) fn select_repo(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.active.as_ref().is_some_and(|r| r.path() == path) {
            return;
        }
        match data::open_repo(&path) {
            Ok(repo) => self.activate_repo(repo, cx),
            Err(e) => self.set_status(format!("could not open {}: {e}", path.display()), true),
        }
        cx.notify();
    }

    pub(super) fn on_copy_path(
        &mut self,
        _: &CopyPath,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.selected else {
            return;
        };
        let Some(path) = self.rows.get(ix).map(|r| r.path.clone()) else {
            return;
        };
        self.copy_path_to_clipboard(path, cx);
    }

    pub(super) fn on_open_in_terminal(
        &mut self,
        _: &OpenInTerminal,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.selected else {
            return;
        };
        let Some(path) = self.rows.get(ix).map(|r| r.path.clone()) else {
            return;
        };
        self.open_in_terminal_path(path, cx);
    }

    pub(super) fn on_reveal_in_finder(
        &mut self,
        _: &RevealInFinder,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.selected else {
            return;
        };
        let Some(path) = self.rows.get(ix).map(|r| r.path.clone()) else {
            return;
        };
        self.reveal_path_in_finder(path, cx);
    }

    /// Copy `path` to the clipboard. Shared by the ⌘C binding (which
    /// resolves `path` from `self.selected`) and both context menus' "Copy
    /// Path" item (which already have a path in hand).
    fn copy_path_to_clipboard(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let text = path.display().to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { data::copy_to_clipboard(&text) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.set_status("path copied", false),
                    Err(e) => this.set_status(format!("copy failed: {e}"), true),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Open `path` in a terminal. Shared the same way as
    /// `copy_path_to_clipboard` above.
    fn open_in_terminal_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn({
                    let path = path.clone();
                    async move { data::open_in_terminal(&path) }
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.set_status(format!("opened {} in terminal", path.display()), false)
                    }
                    Err(e) => this.set_status(format!("could not open terminal: {e}"), true),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reveal `path` in Finder. Shared the same way as
    /// `copy_path_to_clipboard` above.
    pub(crate) fn reveal_path_in_finder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn({
                    let path = path.clone();
                    async move { data::reveal_in_finder(&path) }
                })
                .await;
            this.update(cx, |this, cx| {
                if let Err(e) = result {
                    this.set_status(format!("could not reveal in Finder: {e}"), true);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // -------------------------------------------------------------
    // Add repository
    // -------------------------------------------------------------

    /// Open a native folder picker and add whatever repository the user
    /// chooses — the mouse-driven answer to "how do I add more repos (no
    /// plus button)?": the sidebar's `+` button, its empty state, the
    /// `⌘⇧O` binding, and the empty-space context menu's "Add
    /// Repository…" all funnel through this one handler.
    pub(super) fn on_add_repository(
        &mut self,
        _: &AddRepository,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_open() {
            return;
        }
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add".into()),
        });
        cx.spawn(async move |this, cx| {
            // Three ways this resolves to "nothing to do": the oneshot
            // channel was dropped (outer `Err`), the platform call itself
            // failed (inner `Err`), or the user cancelled the picker
            // (`Ok(None)`) — none of them are errors worth a status
            // message, they are all just "never mind".
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| this.finish_add_repository(path, cx))
                .ok();
        })
        .detach();
    }

    /// Resolve `path` to its repository and add it. `activate_repo` already
    /// records the repository in the registry (`wtm::registry::remember`,
    /// called from `begin_activate_repo`) and selects it — exactly the "add
    /// to the registry, then select it" this affordance promises, reusing
    /// the same path a sidebar click already takes rather than duplicating
    /// its registry bookkeeping here.
    ///
    /// `pub(super)` (rather than private) so `integration_tests` can call it
    /// directly: gpui 0.2.2's `TestAppContext` has no way to simulate the
    /// platform's `prompt_for_paths` (unlike `prompt_for_new_path`, which
    /// `simulate_new_path_selection` drives — see that test module's doc
    /// comment on `add_repository_resolves_and_activates_chosen_directory`
    /// for the full explanation), so this is the testable core of "Add
    /// Repository" reached directly, skipping only the picker itself.
    pub(super) fn finish_add_repository(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match data::open_repo(&path) {
            Ok(repo) => self.activate_repo(repo, cx),
            Err(e) => self.set_status(
                format!("{} is not a git repository: {e}", path.display()),
                true,
            ),
        }
        cx.notify();
    }
}

/// A menu item for an action that needs an open repository: shown and
/// carrying its real shortcut when one is open, shown-but-disabled with the
/// reason in the shortcut slot otherwise — the same idiom the worktree row
/// menu's main-worktree `Remove…` already uses, so a right-click on an empty
/// window with nothing open explains itself instead of looking broken.
fn repo_scoped_item(
    has_repo: bool,
    id: &'static str,
    label: &'static str,
    icon: &'static str,
    shortcut: &'static str,
) -> MenuItem {
    let item = MenuItem::action(id, label).icon(icon);
    if has_repo {
        item.shortcut(shortcut)
    } else {
        item.disabled().shortcut("open a repository first")
    }
}

/// The reason "Open on Remote…" is disabled, if it is — pulled out as its
/// own pure function (no git, no `MenuItem`, no `WtmApp`) so it is directly
/// unit testable without a real repository or worktree. `open_remote_menu_item`
/// is the thin, otherwise-untested glue that feeds this `info.branch.is_some()`
/// and `data::remote_branch_url`'s result and turns the answer into a real
/// `MenuItem`.
///
/// `has_branch` false means a detached HEAD (nothing to open at all);
/// `resolved_url` is `None` when `remote_branch_url` found no usable
/// remote — see that function's own doc comment for the two cases it
/// returns `None` for (no configured remote, or a remote URL shape it does
/// not recognize).
fn open_remote_disabled_reason(
    has_branch: bool,
    resolved_url: Option<&str>,
) -> Option<&'static str> {
    if !has_branch {
        return Some("detached HEAD has no branch");
    }
    if resolved_url.is_none() {
        return Some("no remote configured");
    }
    None
}

#[cfg(test)]
mod open_remote_tests {
    use super::open_remote_disabled_reason;

    #[test]
    fn detached_head_is_disabled_with_its_own_reason() {
        assert_eq!(
            open_remote_disabled_reason(false, None),
            Some("detached HEAD has no branch")
        );
        // Even a (nonsensical) resolved URL cannot rescue a detached HEAD —
        // "no branch to open" takes priority.
        assert_eq!(
            open_remote_disabled_reason(false, Some("https://example.com")),
            Some("detached HEAD has no branch")
        );
    }

    #[test]
    fn a_branch_with_no_resolvable_remote_is_disabled() {
        assert_eq!(
            open_remote_disabled_reason(true, None),
            Some("no remote configured")
        );
    }

    #[test]
    fn a_branch_with_a_resolved_url_is_enabled() {
        assert_eq!(
            open_remote_disabled_reason(true, Some("https://github.com/owner/repo/tree/main")),
            None
        );
    }
}

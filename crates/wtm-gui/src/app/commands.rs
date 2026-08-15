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
        // selection never disagree.
        self.select(row_ix, cx);

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
        };
        let items = vec![
            MenuItem::action("open-editor", "Open in Editor").icon(icons::OPEN_EXTERNAL),
            MenuItem::action("open-terminal", "Open in Terminal"),
            MenuItem::action("reveal-finder", "Reveal in Finder"),
            MenuItem::action("copy-path", "Copy Path").icon(icons::COPY),
            MenuItem::separator(),
            remove_item,
        ];

        let target = MenuTarget::Worktree(info.path);
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

    pub(super) fn handle_menu_select(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(target) = self.context_menu_target.take() else {
            return;
        };
        match target {
            MenuTarget::Worktree(path) => self.handle_worktree_menu_action(&path, id, cx),
            MenuTarget::Repo(path) => self.handle_repo_menu_action(&path, id, cx),
        }
    }

    fn handle_worktree_menu_action(&mut self, path: &Path, id: &str, cx: &mut Context<Self>) {
        match id {
            "open-editor" => self.open_path_in_editor(path.to_path_buf(), cx),
            "open-terminal" => self.open_in_terminal_path(path.to_path_buf(), cx),
            "reveal-finder" => self.reveal_path_in_finder(path.to_path_buf(), cx),
            "copy-path" => self.copy_path_to_clipboard(path.to_path_buf(), cx),
            "remove" => {
                if let Some(info) = self.rows.iter().find(|row| row.path == path).cloned() {
                    self.open_remove_dialog_for(info, cx);
                }
            }
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
}

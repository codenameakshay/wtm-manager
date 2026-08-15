//! The lifecycle and background operations behind every modal overlay
//! except the context menu: opening and closing `self.dialog`'s three
//! variants (Create/Remove/Prune), streaming a create's progress,
//! confirming a remove or a prune, the command palette's state machine, and
//! bulk remove (`self.bulk_remove`).
//!
//! This is business logic, not rendering — the on-screen form for the
//! Create/Remove/Prune dialogs and for bulk remove lives in `dialog_forms`
//! instead. The command palette's renderer (`render_palette`) is the one
//! exception, kept here next to `PaletteState` the same way the original
//! `app.rs` kept it next to the palette's wiring rather than relegating it
//! to its "Dialog rendering" section.

use super::*;

impl BulkRemoveState {
    fn new(candidates: Vec<PruneCandidate>) -> Self {
        Self {
            candidates,
            force: false,
            busy: false,
            error: None,
        }
    }
}

impl WtmApp {
    // -------------------------------------------------------------
    // Dialog lifecycle
    // -------------------------------------------------------------

    pub(super) fn on_new_worktree(
        &mut self,
        _: &NewWorktree,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_open() {
            return;
        }
        let Some(repo) = self.active.clone() else {
            self.set_status("open a repository first", true);
            cx.notify();
            return;
        };

        let state = CreateState::new(&repo, window, cx);
        let branch_focus = state.branch_input.focus_handle(cx);
        self.dialog = Some(Dialog::Create(state));
        window.focus(&branch_focus);
        self.load_create_branches(cx);
        cx.notify();
    }

    /// Remove: one target's confirmation for the ordinary single-selection
    /// case, or the bulk-remove confirmation when a multi-selection is
    /// active — the same action either way, since `RemoveSelected` is what
    /// ⌘⌫, the palette's "Remove Worktree" command, and this handler all
    /// share.
    pub(super) fn on_remove_selected(
        &mut self,
        _: &RemoveSelected,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let indices = self.selected_indices();
        if indices.len() > 1 {
            self.open_bulk_remove_dialog(indices, cx);
            return;
        }
        let Some(&ix) = indices.first() else {
            return;
        };
        let Some(info) = self.rows.get(ix).cloned() else {
            return;
        };
        self.open_remove_dialog_for(info, cx);
    }

    /// Open the remove-worktree confirmation for `info`. Shared by the ⌘⌫
    /// binding (which resolves `info` from `self.selected`) and a worktree
    /// row's context menu (which resolves it from the right-clicked path).
    pub(super) fn open_remove_dialog_for(&mut self, info: WorktreeInfo, cx: &mut Context<Self>) {
        if self.overlay_open() {
            return;
        }
        let Some(repo) = self.active.as_ref() else {
            return;
        };
        let state = RemoveState::new(info, &repo.config.prune.protected_branches);
        self.dialog = Some(Dialog::Remove(state));
        cx.notify();
    }

    pub(super) fn on_prune_repo(
        &mut self,
        _: &PruneRepo,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_open() {
            return;
        }
        let Some(repo) = self.active.clone() else {
            self.set_status("open a repository first", true);
            cx.notify();
            return;
        };
        let mut state = PruneState::new();
        state.recompute(&repo, &self.rows);
        self.dialog = Some(Dialog::Prune(state));
        cx.notify();
    }

    pub(super) fn on_close_dialog(
        &mut self,
        _: &CloseDialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_dialog(window, cx);
    }

    /// Close whichever dialog is open, if any. Public within the crate
    /// because [`CreateState::new`] wires it up as the `TextInput` fields'
    /// `Cancel` reaction — Escape while a field is focused reaches the
    /// field's own binding first, never [`CloseDialog`] (see the module doc
    /// on why arrow keys need the same treatment).
    ///
    /// Explicitly refocuses the root when a dialog actually closes, rather
    /// than counting on the reclaim check at the top of `render` to notice:
    /// that check is skipped outright while `self.dialog` is `Some` (see its
    /// comment), and by the time this runs `self.dialog` has already been
    /// cleared for this frame, but whatever had focus inside the dialog
    /// (a `TextInput`, most commonly) is about to be unmounted. Handing
    /// focus back here, deterministically, is what keeps ↑/↓/Enter working
    /// on the list the instant the dialog is gone.
    ///
    /// Also closes the settings sheet, the palette, and the bulk-remove
    /// confirmation — all three share the same Escape binding and the same
    /// "root must refocus explicitly" reasoning, since none of them has a
    /// dialog-shaped `Dialog` variant to hold a stale `TextInput` focus
    /// handle. All are mutually exclusive (see `overlay_open`), so at most
    /// one of these `take`s ever does anything.
    ///
    /// When nothing above was open, Escape falls through to one more,
    /// non-destructive thing: collapsing a multi-selection back to its
    /// anchor row, same as clicking a single row would, but without
    /// requiring the mouse. `on_close_dialog`'s own doc promises Escape
    /// never falls through to a destructive action — clearing a selection
    /// is not one.
    pub(crate) fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let closed_dialog = self.dialog.take().is_some();
        let closed_settings = std::mem::take(&mut self.settings_open);
        let closed_palette = self.palette.take().is_some();
        let closed_bulk_remove = self.bulk_remove.take().is_some();
        if closed_dialog || closed_settings || closed_palette || closed_bulk_remove {
            window.focus(&self.focus_handle);
            cx.notify();
            return;
        }
        if !self.multi_selected.is_empty() {
            self.multi_selected.clear();
            cx.notify();
        }
    }

    // -------------------------------------------------------------
    // Create dialog
    // -------------------------------------------------------------

    fn load_create_branches(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.active.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { data::list_branches(&repo) })
                .await;
            this.update(cx, |this, cx| {
                let error = result.as_ref().err().cloned();
                if let Some(Dialog::Create(state)) = &mut this.dialog {
                    state.branches_loading = false;
                    if let Ok(branches) = result {
                        state.branches = branches;
                    }
                }
                if let Some(e) = error {
                    this.set_status(format!("could not list branches: {e}"), true);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fill the branch field from a picker click. Ignores the click if the
    /// dialog closed (or moved to the progress phase) in the meantime.
    pub(super) fn select_branch_in_create(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Dialog::Create(state)) = &mut self.dialog else {
            return;
        };
        let input = state.branch_input.clone();
        input.update(cx, |input, cx| input.set_value(name, window, cx));
    }

    pub(super) fn toggle_run_setup(&mut self, cx: &mut Context<Self>) {
        if let Some(Dialog::Create(state)) = &mut self.dialog {
            if state.setup_available {
                state.run_setup = !state.run_setup;
            }
        }
        cx.notify();
    }

    /// Submit the create form: switch to the progress phase and kick off
    /// the background create. Wired as both the Create button's click and
    /// the branch/base fields' `Submit` reaction (Enter).
    pub(crate) fn submit_create_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Dialog::Create(state)) = &mut self.dialog else {
            return;
        };
        if !matches!(state.phase, CreatePhase::Form) {
            return;
        }
        let branch = state.branch_input.read(cx).value().trim().to_string();
        if branch.is_empty() {
            return;
        }
        let Some(repo) = self.active.clone() else {
            return;
        };
        let base_text = state.base_input.read(cx).value().trim().to_string();
        let base = (!base_text.is_empty()).then_some(base_text);
        let run_setup = state.run_setup;

        state.start_progress(branch.clone());
        // The form's branch/base fields are unmounted the moment the
        // progress view replaces them, taking whatever had focus (the
        // branch field, ordinarily) with them. Root's own key bindings all
        // guard on `self.dialog.is_some()` (see `on_select_next` and
        // friends), so handing focus back to it here is safe even though
        // the dialog is still open — it just means Escape (bound at the
        // root) keeps closing the dialog while the create streams in,
        // instead of focus dangling on an element that no longer exists.
        window.focus(&self.focus_handle);
        cx.notify();

        // `create_worktree_streaming` is blocking and calls its sink from a
        // background thread, so the sink can never touch `this` directly.
        // Instead it posts onto an mpsc channel; a separate foreground task
        // below drains that channel and is the only thing that ever calls
        // back into the entity.
        let (tx, rx) = mpsc::channel::<StreamMsg>();
        let tx_done = tx.clone();
        cx.background_spawn({
            let repo = repo.clone();
            let branch = branch.clone();
            async move {
                let mut sink = move |event: SetupEvent| {
                    let _ = tx.send(StreamMsg::Event(event));
                };
                let result = data::create_worktree_streaming(
                    &repo,
                    &branch,
                    base.as_deref(),
                    run_setup,
                    &mut sink,
                );
                let _ = tx_done.send(StreamMsg::Done(result));
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            loop {
                let mut batch = Vec::new();
                let mut finished = false;
                // Non-blocking: `try_recv` never stalls this foreground
                // task waiting for the background thread. A blocking
                // `rx.recv()` here would freeze the whole window until the
                // create finished — this loop instead polls on an interval,
                // draining whatever has arrived since the last tick.
                while let Ok(msg) = rx.try_recv() {
                    let is_done = matches!(msg, StreamMsg::Done(_));
                    batch.push(msg);
                    if is_done {
                        finished = true;
                        break;
                    }
                }
                if !batch.is_empty() {
                    let alive = this
                        .update(cx, |this, cx| this.apply_create_stream(batch, cx))
                        .is_ok();
                    if !alive {
                        return;
                    }
                }
                if finished {
                    break;
                }
                Timer::after(Duration::from_millis(16)).await;
            }
        })
        .detach();
    }

    /// Apply a batch of streamed events to the progress log. A no-op if the
    /// dialog was closed (or a new create started) since the batch was
    /// captured — the background create itself is not cancelled by closing
    /// the dialog, but nothing updates for it once no `Progress` phase is
    /// there to receive it.
    fn apply_create_stream(&mut self, batch: Vec<StreamMsg>, cx: &mut Context<Self>) {
        let Some(Dialog::Create(state)) = &mut self.dialog else {
            return;
        };
        let CreatePhase::Progress(progress) = &mut state.phase else {
            return;
        };

        let mut select_branch = None;
        for msg in batch {
            match msg {
                StreamMsg::Event(event) => progress.push_event(event),
                StreamMsg::Done(result) => {
                    match &result {
                        Ok(path) => {
                            progress.destination = Some(path.clone());
                            select_branch = Some(progress.branch.clone());
                        }
                        Err(e) => progress.push_error(e.clone()),
                    }
                    progress.outcome = Some(result);
                }
            }
        }

        cx.notify();

        if let Some(branch) = select_branch {
            self.pending_select = Some(branch);
            self.reload(cx);
        }
    }

    // -------------------------------------------------------------
    // Remove dialog
    // -------------------------------------------------------------

    pub(super) fn toggle_remove_force(&mut self, cx: &mut Context<Self>) {
        if let Some(Dialog::Remove(state)) = &mut self.dialog {
            state.force = !state.force;
        }
        cx.notify();
    }

    pub(super) fn toggle_remove_delete_branch(&mut self, cx: &mut Context<Self>) {
        if let Some(Dialog::Remove(state)) = &mut self.dialog {
            if state.branch_protected.is_none() && state.target.branch.is_some() {
                state.delete_branch = !state.delete_branch;
            }
        }
        cx.notify();
    }

    pub(super) fn confirm_remove_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::Remove(state)) = &mut self.dialog else {
            return;
        };
        if !state.can_confirm() {
            return;
        }
        state.busy = true;
        state.error = None;
        let info = state.target.clone();
        let force = state.force;
        let delete_branch = state.delete_branch && info.branch.is_some();
        cx.notify();

        let Some(repo) = self.active.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let remove_result = cx
                .background_spawn({
                    let repo = repo.clone();
                    let info = info.clone();
                    async move { data::remove_worktree(&repo, &info, force) }
                })
                .await;

            let branch_name = info.branch.clone();
            let branch_result = if remove_result.is_ok() && delete_branch {
                if let Some(branch) = branch_name.clone() {
                    Some(
                        cx.background_spawn({
                            let repo = repo.clone();
                            async move { data::delete_branch(&repo, &branch) }
                        })
                        .await,
                    )
                } else {
                    None
                }
            } else {
                None
            };

            this.update(cx, |this, cx| {
                this.finish_remove_dialog(remove_result, branch_result, branch_name, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Report both outcomes honestly: a worktree removed but a branch
    /// delete that failed is a real, partial result — not a failure to
    /// hide and not a success to overstate.
    fn finish_remove_dialog(
        &mut self,
        remove_result: Result<(), String>,
        branch_result: Option<Result<(), String>>,
        branch_name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        match remove_result {
            Ok(()) => {
                self.dialog = None;
                let (message, is_error) = match branch_result {
                    None => ("worktree removed".to_string(), false),
                    Some(Ok(())) => (
                        format!(
                            "worktree removed and branch '{}' deleted",
                            branch_name.unwrap_or_default()
                        ),
                        false,
                    ),
                    Some(Err(e)) => (
                        format!("worktree removed, but branch delete failed: {e}"),
                        true,
                    ),
                };
                self.set_status(message, is_error);
                self.reload(cx);
            }
            Err(e) => {
                if let Some(Dialog::Remove(state)) = &mut self.dialog {
                    state.busy = false;
                    state.error = Some(e);
                }
            }
        }
        cx.notify();
    }

    // -------------------------------------------------------------
    // Prune dialog
    // -------------------------------------------------------------

    pub(super) fn toggle_prune_merged(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.active.clone() else {
            return;
        };
        let rows = self.rows.clone();
        if let Some(Dialog::Prune(state)) = &mut self.dialog {
            state.merged = !state.merged;
            state.recompute(&repo, &rows);
        }
        cx.notify();
    }

    pub(super) fn toggle_prune_gone(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.active.clone() else {
            return;
        };
        let rows = self.rows.clone();
        if let Some(Dialog::Prune(state)) = &mut self.dialog {
            state.gone = !state.gone;
            state.recompute(&repo, &rows);
        }
        cx.notify();
    }

    pub(super) fn toggle_prune_force(&mut self, cx: &mut Context<Self>) {
        if let Some(Dialog::Prune(state)) = &mut self.dialog {
            state.force = !state.force;
        }
        cx.notify();
    }

    pub(super) fn confirm_prune_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::Prune(state)) = &mut self.dialog else {
            return;
        };
        if state.busy || state.candidates.is_empty() {
            return;
        }
        state.busy = true;
        let candidates = state.candidates.clone();
        let force = state.force;
        cx.notify();

        let Some(repo) = self.active.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let report = cx
                .background_spawn(async move { data::run_prune(&repo, &candidates, force) })
                .await;
            this.update(cx, |this, cx| this.finish_prune_dialog(report, cx))
                .ok();
        })
        .detach();
    }

    /// Report the full `PruneReport` honestly: never claim success when
    /// `failures` is non-empty, and name what was skipped for being dirty.
    fn finish_prune_dialog(&mut self, report: PruneReport, cx: &mut Context<Self>) {
        self.dialog = None;
        let mut parts = vec![format!(
            "pruned {} worktree{}",
            report.removed,
            if report.removed == 1 { "" } else { "s" }
        )];
        if !report.skipped.is_empty() {
            parts.push(format!("skipped (dirty): {}", report.skipped.join(", ")));
        }
        let has_failures = !report.failures.is_empty();
        if has_failures {
            parts.push(format!("failed: {}", report.failures.join("; ")));
        }
        self.set_status(parts.join(" · "), has_failures);
        self.reload(cx);
        cx.notify();
    }

    // -------------------------------------------------------------
    // Command palette
    // -------------------------------------------------------------

    pub(super) fn on_open_palette(
        &mut self,
        _: &OpenPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_open() {
            return;
        }
        let state = PaletteState::new(window, cx);
        let input_focus = state.input.focus_handle(cx);
        self.palette = Some(state);
        window.focus(&input_focus);
        cx.notify();
    }

    pub(crate) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            window.focus(&self.focus_handle);
            cx.notify();
        }
    }

    /// Reset the highlight to the top result — the palette's `Changed`
    /// reaction to every keystroke; see `PaletteState::new`.
    pub(crate) fn palette_reset_highlight(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.palette {
            state.highlighted = 0;
        }
        cx.notify();
    }

    pub(crate) fn palette_set_highlight(&mut self, entry_ix: usize, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.palette {
            state.highlighted = entry_ix;
            cx.notify();
        }
    }

    /// Move the highlight by `delta` (±1), wrapping — the palette's own
    /// `on_key_down` reaction to Up/Down (see `on_palette_key_down`),
    /// since `TextInput` binds neither and `WtmApp`'s own `SelectNext`/
    /// `SelectPrev` are no-ops here (`overlay_open` is true while the
    /// palette is open).
    pub(crate) fn palette_move_highlight(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(state) = &self.palette else {
            return;
        };
        let query = state.input.read(cx).value().to_string();
        let len = palette::compute_results(&query, &self.rows).len();
        if len == 0 {
            return;
        }
        let Some(state) = &mut self.palette else {
            return;
        };
        let next = (state.highlighted as i32 + delta).rem_euclid(len as i32) as usize;
        state.highlighted = next;
        cx.notify();
    }

    /// Activate whichever result is currently highlighted — plain Enter's
    /// reaction (see `PaletteState::new`'s `Submit` handling).
    pub(crate) fn palette_activate_highlighted(
        &mut self,
        open_in_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ix = self.palette.as_ref().map(|p| p.highlighted).unwrap_or(0);
        self.palette_activate(ix, open_in_editor, window, cx);
    }

    /// Activate the result at flat index `entry_ix` in the current query's
    /// results (worktrees then commands — see `palette::compute_results`).
    ///
    /// `open_in_editor` decides what a *worktree* result does: `false`
    /// (plain Enter, a plain click) selects the row and closes the palette
    /// only — jumping to a worktree is the common case, and the palette's
    /// whole point is to get there fast without giving up whatever else is
    /// on screen. `true` (⌘+Enter, a ⌘-click — see `on_palette_key_down`
    /// and `palette::render_entry`) additionally opens it in the editor,
    /// which is one keystroke away for the times that *is* what's wanted,
    /// but never the default: switching away to another app is a bigger,
    /// harder-to-undo interruption than a selection change, so it needs an
    /// explicit modifier rather than happening on the bare keystroke
    /// someone reaches for out of habit. Ignored entirely for a *command*
    /// result — running a command has no "jump vs. open" distinction.
    pub(crate) fn palette_activate(
        &mut self,
        entry_ix: usize,
        open_in_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = &self.palette else {
            return;
        };
        let query = state.input.read(cx).value().to_string();
        let Some(entry) = palette::compute_results(&query, &self.rows)
            .into_iter()
            .nth(entry_ix)
        else {
            return;
        };
        self.close_palette(window, cx);
        match entry {
            palette::PaletteEntry::Worktree { row_ix, .. } => {
                self.select(row_ix, cx);
                if open_in_editor {
                    self.open_row_in_editor(row_ix, cx);
                }
            }
            palette::PaletteEntry::Command { spec, .. } => {
                self.run_palette_command(spec.id, window, cx);
            }
        }
    }

    /// Dispatch a palette command through the exact same `WtmApp::on_*`
    /// method its real keystroke or button already calls — the palette
    /// adds no new behavior of its own, only another way to reach it.
    fn run_palette_command(
        &mut self,
        id: palette::CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match id {
            palette::CommandId::NewWorktree => self.on_new_worktree(&NewWorktree, window, cx),
            palette::CommandId::RemoveWorktree => {
                self.on_remove_selected(&RemoveSelected, window, cx)
            }
            palette::CommandId::Prune => self.on_prune_repo(&PruneRepo, window, cx),
            palette::CommandId::Reload => self.on_reload(&Reload, window, cx),
            palette::CommandId::OpenEditor => self.on_open_selected(&OpenSelected, window, cx),
            palette::CommandId::OpenTerminal => {
                self.on_open_in_terminal(&OpenInTerminal, window, cx)
            }
            palette::CommandId::RevealFinder => {
                self.on_reveal_in_finder(&RevealInFinder, window, cx)
            }
            palette::CommandId::CopyPath => self.on_copy_path(&CopyPath, window, cx),
            palette::CommandId::ToggleSidebar => self.on_toggle_sidebar(&ToggleSidebar, window, cx),
            palette::CommandId::ToggleDetailPanel => {
                self.on_toggle_detail_panel(&ToggleDetailPanel, window, cx)
            }
            palette::CommandId::Settings => self.on_open_settings(&OpenSettings, window, cx),
        }
    }

    /// Raw key handler attached to the palette's card (see
    /// `palette::render`): catches Up/Down, which nothing else does while
    /// the palette's `TextInput` has focus (its own keymap binds neither),
    /// and ⌘+Enter, a keystroke distinct enough from plain Enter that
    /// `TextInput`'s "enter"→`Submit` binding never matches it at all — see
    /// the module doc on `crate::palette` for why this, rather than
    /// `stop_propagation`, is how `crate::context_menu` (and now this)
    /// keep a raw key listener from double-acting with the action-dispatch
    /// system: `WtmApp`'s own bindings already no-op while `overlay_open()`
    /// is true, so nothing needs to be suppressed, only implemented.
    pub(crate) fn on_palette_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette.is_none() {
            return;
        }
        match event.keystroke.key.as_str() {
            "down" => self.palette_move_highlight(1, cx),
            "up" => self.palette_move_highlight(-1, cx),
            "enter" if event.keystroke.modifiers.platform => {
                self.palette_activate_highlighted(true, window, cx);
            }
            _ => {}
        }
    }

    pub(super) fn render_palette(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = &self.palette else {
            return div().into_any_element();
        };
        palette::render(state, &self.rows, theme, cx)
    }

    // -------------------------------------------------------------
    // Bulk remove
    // -------------------------------------------------------------

    /// Open the bulk-remove confirmation for `indices` (row indices into
    /// `self.rows`). `data::selection_candidates` already applies the same
    /// safety filter `wtm prune`'s own candidate selection does (never the
    /// main worktree, never a protected branch), so a selection made up
    /// entirely of those can legitimately produce nothing to confirm.
    fn open_bulk_remove_dialog(&mut self, indices: Vec<usize>, cx: &mut Context<Self>) {
        if self.overlay_open() {
            return;
        }
        let Some(repo) = self.active.clone() else {
            return;
        };
        let rows: Vec<WorktreeInfo> = indices
            .into_iter()
            .filter_map(|ix| self.rows.get(ix).cloned())
            .collect();
        let candidates = data::selection_candidates(&repo, rows);
        if candidates.is_empty() {
            self.set_status(
                "nothing to remove — the selection is only the main worktree and/or protected branches",
                true,
            );
            cx.notify();
            return;
        }
        self.bulk_remove = Some(BulkRemoveState::new(candidates));
        cx.notify();
    }

    pub(super) fn toggle_bulk_remove_force(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.bulk_remove {
            state.force = !state.force;
        }
        cx.notify();
    }

    pub(super) fn confirm_bulk_remove(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.bulk_remove else {
            return;
        };
        if state.busy || state.candidates.is_empty() {
            return;
        }
        state.busy = true;
        state.error = None;
        let candidates = state.candidates.clone();
        let force = state.force;
        cx.notify();

        let Some(repo) = self.active.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let report = cx
                .background_spawn(async move { data::run_prune(&repo, &candidates, force) })
                .await;
            this.update(cx, |this, cx| this.finish_bulk_remove(report, cx))
                .ok();
        })
        .detach();
    }

    /// Report the full `PruneReport` honestly, the same way
    /// `finish_prune_dialog` already does for the single-target Prune
    /// dialog: "some removed, some skipped (dirty), some failed" is a real
    /// outcome for a batch operation, not something to collapse into a
    /// single success/failure. Always closes the confirmation — a partial
    /// result is reported via the status line, not by leaving a modal open
    /// the way the single-target Remove dialog does for its one-item
    /// `Result`.
    fn finish_bulk_remove(&mut self, report: PruneReport, cx: &mut Context<Self>) {
        self.bulk_remove = None;
        self.multi_selected.clear();
        let mut parts = vec![format!(
            "removed {} worktree{}",
            report.removed,
            if report.removed == 1 { "" } else { "s" }
        )];
        if !report.skipped.is_empty() {
            parts.push(format!("skipped (dirty): {}", report.skipped.join(", ")));
        }
        let has_failures = !report.failures.is_empty();
        if has_failures {
            parts.push(format!("failed: {}", report.failures.join("; ")));
        }
        self.set_status(parts.join(" · "), has_failures);
        self.reload(cx);
        cx.notify();
    }
}

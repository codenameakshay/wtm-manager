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
            done: 0,
            error: None,
        }
    }
}

/// Progress of a prune or bulk remove running in the background.
enum PruneMsg {
    /// Candidates dealt with so far.
    Progress(usize),
    Done(PruneReport),
}

/// Which confirmation the running prune belongs to.
#[derive(Clone, Copy)]
enum PruneTarget {
    Dialog,
    BulkRemove,
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
            self.set_error("open a repository first", cx);
            cx.notify();
            return;
        };

        let state = CreateState::new(&repo, window, cx);
        let branch_focus = state.branch_input.focus_handle(cx);
        self.dialog = Some(Dialog::Create(state));
        window.focus(&branch_focus);
        self.load_create_branches(cx);
        self.load_create_refs(cx);
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let indices = self.selected_indices();
        if indices.len() > 1 {
            self.open_bulk_remove_dialog(indices, window, cx);
            return;
        }
        let Some(&ix) = indices.first() else {
            return;
        };
        let Some(info) = self.rows.get(ix).cloned() else {
            return;
        };
        self.open_remove_dialog_for(info, window, cx);
    }

    /// Open the remove-worktree confirmation for `info`. Shared by the ⌘⌫
    /// binding (which resolves `info` from `self.selected`) and a worktree
    /// row's context menu (which resolves it from the right-clicked path).
    ///
    /// This dialog has no text field, so focus lands on
    /// `self.dialog_safe_focus` (tracked by the Cancel button in
    /// `dialog_forms::render_remove_dialog`) — the safe action, never the
    /// destructive `Remove` button.
    pub(super) fn open_remove_dialog_for(
        &mut self,
        info: WorktreeInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_open() {
            return;
        }
        let Some(repo) = self.active.as_ref() else {
            return;
        };
        let state = RemoveState::new(info, &repo.config.prune.protected_branches);
        self.dialog = Some(Dialog::Remove(state));
        window.focus(&self.dialog_safe_focus);
        cx.notify();
    }

    /// This dialog has no text field either — see `open_remove_dialog_for`'s
    /// doc on why focus lands on `self.dialog_safe_focus` (Cancel) rather
    /// than the destructive `Prune` button.
    pub(super) fn on_prune_repo(
        &mut self,
        _: &PruneRepo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_open() {
            return;
        }
        let Some(repo) = self.active.clone() else {
            self.set_error("open a repository first", cx);
            cx.notify();
            return;
        };
        let mut state = PruneState::new();
        state.recompute(&repo, &self.rows);
        self.dialog = Some(Dialog::Prune(state));
        window.focus(&self.dialog_safe_focus);
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
        let closed_settings = self.settings_open;
        self.settings_open = false;
        let closed_palette = self.palette.take().is_some();
        let closed_bulk_remove = self.bulk_remove.take().is_some();
        // Taking `run_command` here does not stop whatever command is still
        // running in the background — see `crate::run_panel`'s module doc
        // ("The child process outlives a closed dialog") for the full
        // explanation of what that means and why it's the same tradeoff the
        // create dialog already makes for its own setup commands.
        let closed_run_command = self.run_command.take().is_some();
        if closed_dialog
            || closed_settings
            || closed_palette
            || closed_bulk_remove
            || closed_run_command
        {
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
                    this.set_error(format!("could not list branches: {e}"), cx);
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

    /// Load the refs the Base field's picker offers, mirroring
    /// `load_create_branches` above. `current_worktree` is whichever
    /// worktree row is selected in the main list when the dialog opens — the
    /// worktree the user is "currently looking at" for `RefKind::Current`'s
    /// purposes (see `data::list_refs`'s doc comment) — or `None` if nothing
    /// is selected, in which case no ref becomes `Current`.
    fn load_create_refs(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.active.clone() else {
            return;
        };
        let current_worktree = self.selected_worktree_path();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(
                    async move { data::list_refs(&repo, current_worktree.as_deref()) },
                )
                .await;
            this.update(cx, |this, cx| {
                let error = result.as_ref().err().cloned();
                if let Some(Dialog::Create(state)) = &mut this.dialog {
                    state.base_refs_loading = false;
                    if let Ok(refs) = result {
                        state.base_refs = refs;
                    }
                }
                if let Some(e) = error {
                    this.set_error(format!("could not list refs: {e}"), cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fill the Base field from a picker click or an Enter on the
    /// highlighted row, and close the picker — a pick is a complete answer,
    /// not something that leaves the dropdown open waiting for a second
    /// action. Ignores the pick if the dialog closed in the meantime, the
    /// same guard `select_branch_in_create` uses.
    pub(super) fn select_base_ref_in_create(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Dialog::Create(state)) = &mut self.dialog else {
            return;
        };
        let input = state.base_input.clone();
        input.update(cx, |input, cx| input.set_value(name, window, cx));
        state.base_picker_open = false;
        cx.notify();
    }

    /// Show the Base field's ref picker — `CreateState::new`'s reaction to
    /// `base_input` gaining focus. A no-op once the dialog has moved past
    /// its form phase (or closed entirely), which a late-firing focus event
    /// can still deliver after the user has already submitted or cancelled.
    pub(crate) fn open_base_picker(&mut self, cx: &mut Context<Self>) {
        if let Some(Dialog::Create(state)) = &mut self.dialog {
            state.base_picker_open = true;
            state.base_picker_highlight = 0;
        }
        cx.notify();
    }

    /// Hide the Base field's ref picker without touching focus or the
    /// field's typed value — used both by `base_input` losing focus (the
    /// user moved to another field) and, deliberately, by Escape (see
    /// `close_base_picker_or_dialog`), which must not also blur the field:
    /// the whole point of the picker doubling as free-text entry is that
    /// dismissing it with Escape leaves the user right where they were,
    /// mid-edit, not kicked out of the field.
    pub(crate) fn close_base_picker(&mut self, cx: &mut Context<Self>) {
        if let Some(Dialog::Create(state)) = &mut self.dialog {
            state.base_picker_open = false;
        }
        cx.notify();
    }

    /// Mouse-hover reaction for a picker row, mirroring
    /// `palette_set_highlight` — hovering a row moves the keyboard highlight
    /// to it, so mouse and keyboard navigation never disagree about which
    /// row Enter would pick.
    pub(super) fn set_base_picker_highlight(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(Dialog::Create(state)) = &mut self.dialog {
            state.base_picker_highlight = ix;
        }
        cx.notify();
    }

    /// The Base field's `Submit` reaction (Enter). While the picker is
    /// closed this is exactly `submit_create_dialog`, the same as the
    /// branch field's Enter — the common case of typing a ref by hand and
    /// hitting Enter to create the worktree. While the picker is *open*,
    /// Enter means something narrower: pick whatever's highlighted, if
    /// anything is (mirrors clicking a row). If nothing is highlighted —
    /// the filtered list is empty, e.g. a sha the picker has no matching row
    /// for — Enter instead just closes the picker, the explicit "use
    /// exactly what I typed" affordance: the field's raw text is left
    /// untouched, and a second Enter (picker now closed) submits it as the
    /// base, verbatim.
    pub(crate) fn submit_create_or_pick_base(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Dialog::Create(state)) = &self.dialog else {
            return;
        };
        if !state.base_picker_open {
            self.submit_create_dialog(window, cx);
            return;
        }
        let query = state.base_input.read(cx).value().to_string();
        let filtered = dialogs::filter_refs(&state.base_refs, &query);
        let highlighted = dialogs::clamp_highlight(state.base_picker_highlight, filtered.len());
        let picked = filtered.get(highlighted).map(|r| r.name.clone());
        match picked {
            Some(name) => self.select_base_ref_in_create(name, window, cx),
            None => self.close_base_picker(cx),
        }
    }

    /// The Base field's `Cancel` reaction (Escape). While the picker is
    /// open, Escape closes *only* the picker — this is what keeps Escape
    /// from also closing the whole create dialog out from under someone who
    /// only meant to dismiss the suggestion list; see `close_base_picker`'s
    /// doc comment for why that also leaves focus and the typed value
    /// alone. Only once the picker is already closed does Escape fall
    /// through to the ordinary dialog-wide behavior every other field's
    /// Cancel already has.
    pub(crate) fn close_base_picker_or_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let picker_open =
            matches!(&self.dialog, Some(Dialog::Create(state)) if state.base_picker_open);
        if picker_open {
            self.close_base_picker(cx);
        } else {
            self.close_dialog(window, cx);
        }
    }

    /// Raw key handler on the create dialog's card, catching Up/Down for the
    /// Base field's picker — like the palette's search field (see
    /// `crate::palette`'s module doc), `TextInput`'s own keymap binds
    /// neither, so nothing would move the highlight without this. A no-op
    /// whenever the picker isn't open, so it never interferes with the
    /// branch field or any other key handling in the dialog; no
    /// `stop_propagation()` needed either, for the same reason
    /// `on_palette_key_down` doesn't need one — `WtmApp`'s own `SelectNext`/
    /// `SelectPrev` (also bound to Up/Down, at the root) already no-op while
    /// `overlay_open()` is true, which it is for as long as this dialog is
    /// open.
    pub(crate) fn on_create_dialog_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Dialog::Create(state)) = &self.dialog else {
            return;
        };
        if !state.base_picker_open {
            return;
        }
        let delta = match event.keystroke.key.as_str() {
            "down" => 1,
            "up" => -1,
            _ => return,
        };
        let query = state.base_input.read(cx).value().to_string();
        let len = dialogs::filter_refs(&state.base_refs, &query).len();

        let Some(Dialog::Create(state)) = &mut self.dialog else {
            return;
        };
        state.base_picker_highlight =
            dialogs::move_highlight(state.base_picker_highlight, delta, len);
        cx.notify();
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
        // instead of focus dangling on an unmounted element.
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

        Self::drain_stream(
            cx,
            rx,
            |msg| matches!(msg, StreamMsg::Done(_)),
            |this, batch, cx| this.apply_create_stream(batch, cx),
        );
    }

    /// Drain an mpsc stream of background-thread messages onto the
    /// foreground entity, batching whatever has already queued up before
    /// each apply. Shared by the create dialog's progress stream and the
    /// run-command dialog's output stream — both need the same
    /// TestDispatcher-safe polling (a blocking std receiver would stall the
    /// cooperative test executor) and the same "batch until Done, apply,
    /// stop once the entity or the stream is gone" shape.
    fn drain_stream<M: Send + 'static>(
        cx: &mut Context<Self>,
        rx: mpsc::Receiver<M>,
        is_done: fn(&M) -> bool,
        apply: impl Fn(&mut WtmApp, Vec<M>, &mut Context<WtmApp>) + Send + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            #[cfg(not(test))]
            let mut rx = rx;
            #[cfg(test)]
            let rx = rx;
            loop {
                #[cfg(test)]
                let first = loop {
                    match rx.try_recv() {
                        Ok(msg) => break Result::<M, mpsc::RecvError>::Ok(msg),
                        Err(mpsc::TryRecvError::Disconnected) => return,
                        Err(mpsc::TryRecvError::Empty) => {}
                    }
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(16))
                        .await;
                };
                #[cfg(not(test))]
                let first = {
                    let (first, returned_rx) = cx
                        .background_spawn(async move {
                            let first = rx.recv();
                            (first, rx)
                        })
                        .await;
                    rx = returned_rx;
                    first
                };
                let Ok(first) = first else {
                    return;
                };
                let mut batch = vec![first];
                while let Ok(msg) = rx.try_recv() {
                    let done = is_done(&msg);
                    batch.push(msg);
                    if done {
                        break;
                    }
                }
                let finished = batch.iter().any(is_done);
                let alive = this.update(cx, |this, cx| apply(this, batch, cx)).is_ok();
                if !alive {
                    return;
                }
                if finished {
                    break;
                }
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
                if is_error {
                    self.set_error(message, cx);
                } else {
                    self.set_info(message, cx);
                }
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
        if let Some(Dialog::Prune(state)) = &mut self.dialog {
            state.merged = !state.merged;
            state.recompute(&repo, &self.rows);
        }
        cx.notify();
    }

    pub(super) fn toggle_prune_gone(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.active.clone() else {
            return;
        };
        if let Some(Dialog::Prune(state)) = &mut self.dialog {
            state.gone = !state.gone;
            state.recompute(&repo, &self.rows);
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
        state.done = 0;
        let candidates = state.candidates.clone();
        let force = state.force;
        self.start_prune(candidates, force, PruneTarget::Dialog, cx);
    }

    /// Run the prune in the background, streaming "n of N" progress back
    /// and reloading exactly once when it finishes (see `prune_in_flight`).
    fn start_prune(
        &mut self,
        candidates: Vec<PruneCandidate>,
        force: bool,
        target: PruneTarget,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
        let Some(repo) = self.active.clone() else {
            return;
        };
        self.prune_in_flight = true;
        let (tx, rx) = mpsc::channel::<PruneMsg>();
        cx.background_spawn(async move {
            let progress = |done| {
                let _ = tx.send(PruneMsg::Progress(done));
            };
            let report = data::run_prune(&repo, &candidates, force, &progress);
            let _ = tx.send(PruneMsg::Done(report));
        })
        .detach();
        Self::drain_stream(
            cx,
            rx,
            |msg| matches!(msg, PruneMsg::Done(_)),
            move |this, batch, cx| this.apply_prune_stream(batch, target, cx),
        );
    }

    fn apply_prune_stream(
        &mut self,
        batch: Vec<PruneMsg>,
        target: PruneTarget,
        cx: &mut Context<Self>,
    ) {
        for msg in batch {
            match msg {
                PruneMsg::Progress(done) => match target {
                    PruneTarget::Dialog => {
                        if let Some(Dialog::Prune(state)) = &mut self.dialog {
                            state.done = done;
                        }
                    }
                    PruneTarget::BulkRemove => {
                        if let Some(state) = &mut self.bulk_remove {
                            state.done = done;
                        }
                    }
                },
                PruneMsg::Done(report) => {
                    self.prune_in_flight = false;
                    match target {
                        PruneTarget::Dialog => self.finish_prune_dialog(report, cx),
                        PruneTarget::BulkRemove => self.finish_bulk_remove(report, cx),
                    }
                }
            }
        }
        cx.notify();
    }

    fn finish_prune_dialog(&mut self, report: PruneReport, cx: &mut Context<Self>) {
        self.dialog = None;
        self.report_prune("pruned", report, cx);
    }

    /// Report a `PruneReport` honestly: never claim success when `failures`
    /// is non-empty, and name what was skipped for being dirty. Shared by
    /// the Prune dialog and bulk remove, which differ only in the verb.
    fn report_prune(&mut self, verb: &str, report: PruneReport, cx: &mut Context<Self>) {
        let mut parts = vec![format!(
            "{verb} {} worktree{}",
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
        let message = parts.join(" · ");
        if has_failures {
            self.set_error(message, cx);
        } else {
            self.set_info(message, cx);
        }
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
        let results = palette::compute_results(&query, &self.rows);
        if results.is_empty() {
            return;
        }
        let worktree_count = results
            .iter()
            .filter(|e| matches!(e, palette::PaletteEntry::Worktree { .. }))
            .count();
        let command_count = results.len() - worktree_count;
        let Some(state) = &mut self.palette else {
            return;
        };
        state.highlighted = dialogs::move_highlight(state.highlighted, delta, results.len());
        state.scroll_highlighted_into_view(worktree_count, command_count);
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
            palette::CommandId::FetchRemote => self.on_fetch_remote(&FetchRemote, window, cx),
            palette::CommandId::AddRepository => self.on_add_repository(&AddRepository, window, cx),
            palette::CommandId::ShowDetailsTab => {
                self.on_show_details_tab(&ShowDetailsTab, window, cx)
            }
            palette::CommandId::ShowFilesTab => self.on_show_files_tab(&ShowFilesTab, window, cx),
            palette::CommandId::ShowChangesTab => {
                self.on_show_changes_tab(&ShowChangesTab, window, cx)
            }
            palette::CommandId::RunCommand => self.on_run_command(&RunCommand, window, cx),
            palette::CommandId::OpenRemote => self.open_remote_selected(window, cx),
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
    ///
    /// No text field here either — see `open_remove_dialog_for`'s doc on
    /// why focus lands on `self.dialog_safe_focus` (Cancel).
    fn open_bulk_remove_dialog(
        &mut self,
        indices: Vec<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            self.set_error(
                "nothing to remove — the selection is only the main worktree and/or protected branches",
                cx,
            );
            cx.notify();
            return;
        }
        self.bulk_remove = Some(BulkRemoveState::new(candidates));
        window.focus(&self.dialog_safe_focus);
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
        state.done = 0;
        let candidates = state.candidates.clone();
        let force = state.force;
        self.start_prune(candidates, force, PruneTarget::BulkRemove, cx);
    }

    /// Always closes the confirmation — a partial result is reported via
    /// the status line, not by leaving a modal open the way the
    /// single-target Remove dialog does for its one-item `Result`.
    fn finish_bulk_remove(&mut self, report: PruneReport, cx: &mut Context<Self>) {
        self.bulk_remove = None;
        self.multi_selected.clear();
        self.report_prune("removed", report, cx);
    }

    // -------------------------------------------------------------
    // Run command dialog
    // -------------------------------------------------------------

    /// ⌘E: open the Run Command dialog for the selected worktree. A no-op
    /// with nothing selected, the same guard `on_copy_path`/
    /// `on_open_in_terminal` already use for a single-target action.
    pub(super) fn on_run_command(
        &mut self,
        _: &RunCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(info) = self.selected_row().cloned() else {
            return;
        };
        self.open_run_command_dialog(info, window, cx);
    }

    /// Open the Run Command dialog for `info`. Shared by `on_run_command`
    /// (which resolves `info` from `self.selected`) and a worktree row's
    /// context menu (which already has one) — the same split
    /// `open_remove_dialog_for` uses for the Remove dialog.
    pub(super) fn open_run_command_dialog(
        &mut self,
        info: WorktreeInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_open() {
            return;
        }
        let state = RunCommandState::new(info, window, cx);
        let input_focus = state.command_input.focus_handle(cx);
        self.run_command = Some(state);
        window.focus(&input_focus);
        cx.notify();
    }

    /// Fill the command field from a recent-command suggestion click.
    /// Ignores the click if the dialog closed (or moved to the running
    /// phase) in the meantime — the same guard `select_branch_in_create`
    /// uses.
    pub(crate) fn select_recent_command(
        &mut self,
        command: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = &self.run_command else {
            return;
        };
        if !matches!(state.phase, run_panel::RunPhase::Form) {
            return;
        }
        let input = state.command_input.clone();
        input.update(cx, |input, cx| input.set_value(command, window, cx));
    }

    /// Submit the command form: switch to the running phase and kick off
    /// the background run. Wired as both the Run button's click and the
    /// command field's `Submit` reaction (Enter).
    ///
    /// Crosses the background/foreground boundary the same way
    /// `submit_create_dialog` does for the create dialog's own streaming
    /// progress view — see that method's doc comment (and
    /// `crate::run_panel`'s module doc) for why this uses an mpsc channel
    /// plus a foreground drain loop rather than calling back into `self`
    /// straight from `data::run_command_streaming`'s sink (which runs on a
    /// background thread and cannot touch `this`). The drain waits for each
    /// first message on a background executor and batches any messages already
    /// queued before applying them on the foreground.
    pub(crate) fn submit_run_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = &mut self.run_command else {
            return;
        };
        if !matches!(state.phase, run_panel::RunPhase::Form) {
            return;
        }
        let command = state.command_input.read(cx).value().trim().to_string();
        if command.is_empty() {
            return;
        }
        let worktree_path = state.target.path.clone();

        state.start_running(command.clone());
        // The form's command field is unmounted the moment the running
        // phase replaces it, taking its focus with it — hand focus back to
        // the root explicitly, the same reasoning
        // `submit_create_dialog` documents for the identical situation.
        window.focus(&self.focus_handle);
        cx.notify();

        if let Some(repo_key) = self.active.as_ref().map(|r| r.path().to_path_buf()) {
            let recent = self.recent_commands.entry(repo_key).or_default();
            run_panel::record_recent_command(recent, command.clone(), run_panel::MAX_RECENT_STORED);
        }

        let (tx, rx) = mpsc::channel::<run_panel::RunStreamMsg>();
        let tx_done = tx.clone();
        cx.background_spawn({
            let command = command.clone();
            async move {
                let mut sink = move |event: data::CommandEvent| {
                    let _ = tx.send(run_panel::RunStreamMsg::Event(event));
                };
                let result = data::run_command_streaming(&worktree_path, &command, &mut sink);
                let _ = tx_done.send(run_panel::RunStreamMsg::Done(result));
            }
        })
        .detach();

        Self::drain_stream(
            cx,
            rx,
            |msg| matches!(msg, run_panel::RunStreamMsg::Done(_)),
            |this, batch, cx| this.apply_run_command_stream(batch, cx),
        );
    }

    /// Apply a batch of streamed events to the running view. A no-op if the
    /// dialog was closed (or a new run started) since the batch was
    /// captured — the background command itself is not cancelled by closing
    /// the dialog, but nothing updates for it once no `Running` phase is
    /// there to receive it. See `crate::run_panel`'s module doc for what
    /// that means for the child process.
    fn apply_run_command_stream(
        &mut self,
        batch: Vec<run_panel::RunStreamMsg>,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = &mut self.run_command else {
            return;
        };
        let run_panel::RunPhase::Running(progress) = &mut state.phase else {
            return;
        };

        for msg in batch {
            match msg {
                run_panel::RunStreamMsg::Event(data::CommandEvent::Started { .. }) => {}
                run_panel::RunStreamMsg::Event(data::CommandEvent::Output { line }) => {
                    progress.push_line(line);
                }
                run_panel::RunStreamMsg::Event(data::CommandEvent::Finished { success, code }) => {
                    progress.outcome = Some(run_panel::RunOutcome::Finished { success, code });
                }
                run_panel::RunStreamMsg::Done(Ok(())) => {}
                run_panel::RunStreamMsg::Done(Err(e)) => {
                    // Only reachable when the command could never be
                    // started at all (`data::run_command_streaming`'s one
                    // real `Err` case) — if `Finished` already landed, this
                    // `Done` is just the ordinary `Ok(())` tail, never an
                    // `Err`, so this branch cannot overwrite a real outcome.
                    if progress.outcome.is_none() {
                        progress.outcome = Some(run_panel::RunOutcome::StartFailed(e));
                    }
                }
            }
        }

        cx.notify();
    }

    pub(super) fn render_run_command_dialog(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(state) = &self.run_command else {
            return div().into_any_element();
        };
        let recent: &[String] = self
            .active
            .as_ref()
            .and_then(|repo| self.recent_commands.get(repo.path()))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        run_panel::render(state, recent, theme, cx)
    }
}

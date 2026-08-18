//! Rendering for the Create, Remove, and Prune dialogs, plus the
//! bulk-remove confirmation: the "Dialog rendering" section the original
//! `app.rs` already kept separate from those dialogs' lifecycle logic
//! (which lives in `dialog_actions`). Split into its own file, rather than
//! folded into `chrome`, purely to keep both files a reasonable size —
//! see `chrome`'s own doc comment for the rest of that reasoning.
//!
//! Nothing here mutates `WtmApp` state directly except through the
//! `cx.listener` callbacks wired to `dialog_actions`' methods.

use super::*;
use crate::motion;
use crate::theme::{RADIUS_CONTROL, SPACE_12, SPACE_16, SPACE_2, SPACE_4, SPACE_6, SPACE_8};
use crate::ui::{TEXT_BASE, TEXT_SM, TEXT_XS};

impl WtmApp {
    // -------------------------------------------------------------
    // Dialog rendering
    // -------------------------------------------------------------

    pub(super) fn render_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = Theme::of(cx);
        match self.dialog.as_ref()? {
            Dialog::Create(state) => {
                let repo = self.active.as_ref()?;
                Some(self.render_create_dialog(state, repo, &theme, cx))
            }
            Dialog::Remove(state) => Some(self.render_remove_dialog(state, &theme, cx)),
            Dialog::Prune(state) => Some(self.render_prune_dialog(state, &theme, cx)),
        }
    }

    fn render_create_dialog(
        &self,
        state: &CreateState,
        repo: &OpenRepo,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body: AnyElement = match &state.phase {
            CreatePhase::Form => self.render_create_form(state, theme, cx).into_any_element(),
            CreatePhase::Progress(progress) => self
                .render_create_progress(progress, theme, cx)
                .into_any_element(),
        };

        let card = ui::modal_card(440.0, theme)
            .id("create-dialog-card")
            .on_click(|_, _, cx| cx.stop_propagation())
            // Catches Up/Down for the Base field's ref picker — see
            // `WtmApp::on_create_dialog_key_down`'s doc comment for
            // why this lives here rather than on `TextInput`'s own
            // keymap.
            .on_key_down(cx.listener(WtmApp::on_create_dialog_key_down))
            .child(ui::modal_header(
                "New Worktree",
                Some(&format!("in {}", repo.name())),
                theme,
            ))
            .child(body);

        // SURFACES §7: the card enters with `DIALOG_IN`, the scrim behind it
        // with the cheaper `FADE_QUICK` — two independently animated layers,
        // not one flat cross-fade.
        let backdrop =
            render_modal_backdrop(cx).child(motion::dialog_in("create-dialog-in", card, cx));
        motion::fade_quick("create-dialog-backdrop-in", backdrop, cx).into_any_element()
    }

    fn render_create_form(
        &self,
        state: &CreateState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let query = state.branch_input.read(cx).value().to_string();
        let filtered = dialogs::filter_branches(&state.branches, &query);
        let can_submit = !query.trim().is_empty();

        div()
            .flex()
            .flex_col()
            .gap(px(SPACE_12))
            .px(px(SPACE_16))
            .py(px(SPACE_12))
            .child(labeled_field("Branch", state.branch_input.clone(), theme))
            .child(self.render_base_field(state, theme, cx))
            .child(
                div()
                    .id("create-branch-list")
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_2))
                    .max_h(px(160.0))
                    .overflow_y_scroll()
                    .children(if state.branches_loading {
                        vec![dialog_hint("loading branches…", theme).into_any_element()]
                    } else if filtered.is_empty() {
                        vec![dialog_hint("no matching branches", theme).into_any_element()]
                    } else {
                        filtered
                            .iter()
                            .map(|branch| {
                                let row = dialogs::render_branch_row(branch, theme);
                                if branch.is_checked_out {
                                    row.into_any_element()
                                } else {
                                    let name = branch.name.clone();
                                    row.on_click(cx.listener(move |this, _, window, cx| {
                                        this.select_branch_in_create(name.clone(), window, cx);
                                    }))
                                    .into_any_element()
                                }
                            })
                            .collect()
                    }),
            )
            .child({
                let toggle = dialogs::render_toggle(
                    "create-run-setup",
                    "Run setup commands",
                    state.run_setup,
                    !state.setup_available,
                    theme,
                );
                if state.setup_available {
                    toggle
                        .on_click(cx.listener(|this, _, _window, cx| this.toggle_run_setup(cx)))
                        .into_any_element()
                } else {
                    toggle.into_any_element()
                }
            })
            .when(!state.setup_available, |this| {
                this.child(
                    div()
                        .text_size(px(TEXT_XS))
                        .text_color(theme.text_ghost)
                        .child("This repo configures no setup commands or files to copy."),
                )
            })
            .child(
                ui::modal_footer(theme)
                    .child(
                        ui::button("create-cancel", "Cancel", ButtonVariant::Secondary, theme)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_dialog(window, cx)),
                            ),
                    )
                    .child({
                        let button =
                            ui::button("create-confirm", "Create", ButtonVariant::Primary, theme);
                        if can_submit {
                            button
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_create_dialog(window, cx)
                                }))
                                .into_any_element()
                        } else {
                            button.opacity(0.4).into_any_element()
                        }
                    }),
            )
    }

    /// The Base field: its label, the `TextInput` itself (doubling as the
    /// picker's search field — see `dialogs::CreateState`'s doc comment on
    /// why this dialog uses one field for both free-text entry and
    /// filtering rather than a second, dedicated search box), and, while
    /// `state.base_picker_open` is true, the floating ref picker directly
    /// beneath it.
    fn render_base_field(
        &self,
        state: &CreateState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        labeled_field("Base", state.base_input.clone(), theme)
            .when(state.base_picker_open, |this| {
                this.child(self.render_base_picker(state, theme, cx))
            })
    }

    /// The Base field's ref picker itself: `ui::popover` — the same
    /// `surface_overlay` + `shadow_popover` treatment the command palette
    /// and context menu use for "this sits above the rest of the dialog"
    /// (COMPONENTS.md: elevation is `shadow_*` + `border`/`border_strong`
    /// from the theme, never gpui's built-in `shadow_lg()` ramp, which
    /// isn't tuned for this palette) — so it reads as a floating result
    /// list even though (see the module doc below) it's laid out in
    /// ordinary flow directly under the field rather than absolutely
    /// positioned over what comes after it.
    ///
    /// Rendered inline, in the dialog's normal flex column, rather than via
    /// `gpui::deferred`/`.absolute()` the way the palette and context menu
    /// float over the *whole window*: those both escape their own view
    /// entirely, which is exactly what an in-dialog dropdown must NOT do
    /// here — it needs to stay clipped to, and scroll with, this same
    /// modal card. Absolutely positioning it over the branch list/setup
    /// toggle/footer that follow would need a pixel-accurate anchor this
    /// module has no cheap way to compute (no bounds query is available at
    /// render time), so this pushes that later content down instead — a
    /// deliberate compromise over the reference design's true floating
    /// overlay, traded for a z-order bug this dialog cannot end up with.
    fn render_base_picker(
        &self,
        state: &CreateState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let query = state.base_input.read(cx).value().to_string();
        let filtered = dialogs::filter_refs(&state.base_refs, &query);
        let highlighted = dialogs::clamp_highlight(state.base_picker_highlight, filtered.len());

        ui::popover(theme)
            .id("base-ref-picker")
            .mt(px(SPACE_4))
            .gap(px(SPACE_2))
            .max_h(px(220.0))
            .overflow_y_scroll()
            .p(px(SPACE_4))
            .children(if state.base_refs_loading {
                vec![dialog_hint("loading refs…", theme).into_any_element()]
            } else if filtered.is_empty() {
                vec![dialog_hint(
                    "no matching refs — press Enter to use what you typed",
                    theme,
                )
                .into_any_element()]
            } else {
                filtered
                    .iter()
                    .enumerate()
                    .map(|(ix, r)| {
                        let name = r.name.clone();
                        dialogs::render_ref_row(r, ix == highlighted, theme)
                            .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                                if *hovered {
                                    this.set_base_picker_highlight(ix, cx);
                                }
                            }))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_base_ref_in_create(name.clone(), window, cx);
                            }))
                            .into_any_element()
                    })
                    .collect()
            })
    }

    fn render_create_progress(
        &self,
        progress: &ProgressState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(SPACE_12))
            .px(px(SPACE_16))
            .py(px(SPACE_12))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_2))
                    .child(
                        div()
                            .text_size(px(TEXT_BASE))
                            .text_color(theme.text)
                            .child(format!("Creating '{}'…", progress.branch)),
                    )
                    .when_some(progress.destination.as_ref(), |this, dest| {
                        this.child(ui::meta(icons::FOLDER, dest.display().to_string(), theme))
                    }),
            )
            .child(
                // SURFACES §7: progress/log views are mono on
                // `surface_inset`, newest line pinned in view. The mono face
                // itself lives on each `render_log_entry` line (its own
                // doc), so this well only owns the surface/radius.
                div()
                    .id("create-log")
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_2))
                    .h(px(200.0))
                    .overflow_y_scroll()
                    .track_scroll(&progress.scroll)
                    .px(px(SPACE_8))
                    .py(px(SPACE_8))
                    .rounded(px(RADIUS_CONTROL))
                    .bg(theme.surface_inset)
                    .children(
                        progress
                            .log
                            .iter()
                            .map(|entry| dialogs::render_log_entry(entry, theme)),
                    ),
            )
            .when_some(progress.outcome.as_ref(), |this, outcome| match outcome {
                Ok(_) => this,
                Err(e) => this.child(ui::inline_error(
                    format!(
                        "Failed: {e}. If the worktree was already created on disk, it \
                             still exists — this does not undo it."
                    ),
                    theme,
                )),
            })
            .child(self.render_create_progress_footer(progress, theme, cx))
    }

    fn render_create_progress_footer(
        &self,
        progress: &ProgressState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let footer = ui::modal_footer(theme);
        match &progress.outcome {
            None => footer.child(
                div()
                    .text_size(px(TEXT_XS))
                    .text_color(theme.text_ghost)
                    .child("Working…"),
            ),
            Some(Ok(path)) => {
                let path = path.clone();
                footer
                    .child(
                        ui::button("create-close", "Close", ButtonVariant::Secondary, theme)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_dialog(window, cx)),
                            ),
                    )
                    .child(
                        ui::button(
                            "create-open-editor",
                            "Open in Editor",
                            ButtonVariant::Primary,
                            theme,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.close_dialog(window, cx);
                                this.open_path_in_editor(path.clone(), cx);
                            },
                        )),
                    )
            }
            Some(Err(_)) => footer.child(
                ui::button("create-close", "Close", ButtonVariant::Secondary, theme)
                    .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
            ),
        }
    }

    fn render_remove_dialog(
        &self,
        state: &RemoveState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dirty = state.is_dirty();
        let is_main = state.target.is_main;

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_12))
            .px(px(SPACE_16))
            .py(px(SPACE_12))
            .child(ui::meta(
                icons::FOLDER,
                state.target.path.display().to_string(),
                theme,
            ));

        if is_main {
            // Never a filled button and no `Primary` in this dialog at all
            // (SURFACES §7) — the destructive dialogs get a `Danger` commit
            // or, as here, no commit at all. Wrapped in `inline_error` so
            // the refusal is icon-plus-text, not color alone.
            body = body.child(ui::inline_error(
                "The main worktree can't be removed — it's the repository itself.",
                theme,
            ));
        } else {
            if dirty {
                body = body
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(SPACE_6))
                            .child(ui::icon(icons::WARNING, 12.0, theme.warning))
                            .child(
                                div()
                                    .text_size(px(TEXT_SM))
                                    .text_color(theme.warning)
                                    .child("This worktree has uncommitted changes."),
                            ),
                    )
                    .child(
                        dialogs::render_toggle(
                            "remove-force",
                            "Force (discard uncommitted changes)",
                            state.force,
                            false,
                            theme,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| this.toggle_remove_force(cx))),
                    );
            }

            let branch_toggle_disabled =
                state.branch_protected.is_some() || state.target.branch.is_none();
            let delete_branch_row = dialogs::render_toggle(
                "remove-delete-branch",
                "Also delete the branch",
                state.delete_branch,
                branch_toggle_disabled,
                theme,
            );
            body = body.child(if branch_toggle_disabled {
                delete_branch_row.into_any_element()
            } else {
                delete_branch_row
                    .on_click(
                        cx.listener(|this, _, _window, cx| this.toggle_remove_delete_branch(cx)),
                    )
                    .into_any_element()
            });
            if let Some(reason) = &state.branch_protected {
                body = body.child(
                    div()
                        .text_size(px(TEXT_XS))
                        .text_color(theme.text_ghost)
                        .child(reason.clone()),
                );
            }

            if let Some(error) = &state.error {
                body = body.child(ui::inline_error(format!("Remove failed: {error}"), theme));
            }
        }

        // Destructive dialog: never a filled `Primary` here — `Cancel` stays
        // `Secondary`, the commit action is `Danger`, and that's the only
        // fill in the view (SURFACES §7 / COMPONENTS.md's button hierarchy).
        let mut footer = ui::modal_footer(theme).child(
            ui::button("remove-cancel", "Cancel", ButtonVariant::Secondary, theme)
                .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
        );
        if !is_main {
            let confirm = ui::button("remove-confirm", "Remove", ButtonVariant::Danger, theme);
            let confirm = if state.can_confirm() {
                confirm
                    .on_click(cx.listener(|this, _, _window, cx| this.confirm_remove_dialog(cx)))
                    .into_any_element()
            } else {
                confirm.opacity(0.4).into_any_element()
            };
            footer = footer.child(confirm);
        }
        body = body.child(footer);

        let card = ui::modal_card(400.0, theme)
            .id("remove-dialog-card")
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(ui::modal_header(
                "Remove Worktree",
                Some(state.target.display_name()),
                theme,
            ))
            .child(body);
        let backdrop =
            render_modal_backdrop(cx).child(motion::dialog_in("remove-dialog-in", card, cx));
        motion::fade_quick("remove-dialog-backdrop-in", backdrop, cx).into_any_element()
    }

    fn render_prune_dialog(
        &self,
        state: &PruneState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_12))
            .px(px(SPACE_16))
            .py(px(SPACE_12))
            .child(
                dialogs::render_toggle(
                    "prune-merged",
                    "Merged branches",
                    state.merged,
                    false,
                    theme,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.toggle_prune_merged(cx))),
            )
            .child(
                dialogs::render_toggle("prune-gone", "Upstream gone", state.gone, false, theme)
                    .on_click(cx.listener(|this, _, _window, cx| this.toggle_prune_gone(cx))),
            )
            .child(
                dialogs::render_toggle(
                    "prune-force",
                    "Force (include worktrees with uncommitted changes)",
                    state.force,
                    false,
                    theme,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.toggle_prune_force(cx))),
            )
            .child(
                // SURFACES §7: the destructive dialogs give the affected
                // list real room — a proud `surface_raised` plate per row
                // (see `dialogs::render_candidate_row`), not a bare wash.
                div()
                    .id("prune-candidates")
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_4))
                    .max_h(px(220.0))
                    .overflow_y_scroll()
                    .children(if state.candidates.is_empty() {
                        vec![prune_empty_hint(state, theme).into_any_element()]
                    } else {
                        state
                            .candidates
                            .iter()
                            .map(|c| dialogs::render_candidate_row(c, theme).into_any_element())
                            .collect()
                    }),
            )
            .when(!state.candidates.is_empty(), |this| {
                this.child(destructive_count_line(
                    state.candidates.len(),
                    "prune",
                    theme,
                ))
            })
            .child({
                let footer = ui::modal_footer(theme).child(
                    ui::button("prune-cancel", "Cancel", ButtonVariant::Secondary, theme)
                        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
                );
                let confirm = ui::button("prune-confirm", "Prune", ButtonVariant::Danger, theme);
                let confirm = if !state.candidates.is_empty() && !state.busy {
                    confirm
                        .on_click(cx.listener(|this, _, _window, cx| this.confirm_prune_dialog(cx)))
                        .into_any_element()
                } else {
                    confirm.opacity(0.4).into_any_element()
                };
                footer.child(confirm)
            });

        let card = ui::modal_card(420.0, theme)
            .id("prune-dialog-card")
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(ui::modal_header(
                "Prune Worktrees",
                Some("Sweeps missing and already-prunable worktrees; the toggles below add more"),
                theme,
            ))
            .child(body);
        let backdrop =
            render_modal_backdrop(cx).child(motion::dialog_in("prune-dialog-in", card, cx));
        motion::fade_quick("prune-dialog-backdrop-in", backdrop, cx).into_any_element()
    }

    // -------------------------------------------------------------
    // Bulk remove
    // -------------------------------------------------------------

    /// Mirrors `render_remove_dialog`'s shape (folder icon, dirty warning,
    /// force toggle, error line, Cancel/destructive-confirm footer) but for
    /// N candidates instead of one — see `BulkRemoveState`'s doc for why
    /// this cannot just be another `Dialog` variant.
    pub(super) fn render_bulk_remove_dialog(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(state) = &self.bulk_remove else {
            return div().into_any_element();
        };
        let count = state.candidates.len();
        let has_dirty = state
            .candidates
            .iter()
            .any(|c| !c.info.is_missing && c.info.status.as_ref().is_some_and(|s| s.dirty));

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_12))
            .px(px(SPACE_16))
            .py(px(SPACE_12))
            .child(
                div()
                    .text_size(px(TEXT_BASE))
                    .text_color(theme.text_muted)
                    .child(format!(
                        "{count} worktree{} will be removed:",
                        if count == 1 { "" } else { "s" }
                    )),
            );

        if has_dirty {
            body = body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(SPACE_6))
                        .child(ui::icon(icons::WARNING, 12.0, theme.warning))
                        .child(
                            div()
                                .text_size(px(TEXT_SM))
                                .text_color(theme.warning)
                                .child("Some selected worktrees have uncommitted changes."),
                        ),
                )
                .child(
                    dialogs::render_toggle(
                        "bulk-remove-force",
                        "Force (discard uncommitted changes)",
                        state.force,
                        false,
                        theme,
                    )
                    .on_click(
                        cx.listener(|this, _, _window, cx| this.toggle_bulk_remove_force(cx)),
                    ),
                );
        }

        // SURFACES §7: real room for the affected list — a proud
        // `surface_raised` plate per row, same as the Prune dialog's own
        // candidate list.
        body = body.child(
            div()
                .id("bulk-remove-list")
                .flex()
                .flex_col()
                .gap(px(SPACE_4))
                .max_h(px(220.0))
                .overflow_y_scroll()
                .children(
                    state
                        .candidates
                        .iter()
                        .map(|c| dialogs::render_candidate_row(c, theme).into_any_element()),
                ),
        );

        if let Some(error) = &state.error {
            body = body.child(ui::inline_error(format!("Remove failed: {error}"), theme));
        }

        // The count restated in words directly above the action bar — the
        // last thing seen before committing to the destructive action, not
        // just the intro line above the list.
        body = body.child(destructive_count_line(count, "remove", theme));

        let footer = ui::modal_footer(theme).child(
            ui::button(
                "bulk-remove-cancel",
                "Cancel",
                ButtonVariant::Secondary,
                theme,
            )
            .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
        );
        let confirm = ui::button(
            "bulk-remove-confirm",
            "Remove",
            ButtonVariant::Danger,
            theme,
        );
        let confirm = if !state.busy {
            confirm
                .on_click(cx.listener(|this, _, _window, cx| this.confirm_bulk_remove(cx)))
                .into_any_element()
        } else {
            confirm.opacity(0.4).into_any_element()
        };
        body = body.child(footer.child(confirm));

        let card = ui::modal_card(420.0, theme)
            .id("bulk-remove-dialog-card")
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(ui::modal_header(
                "Remove Worktrees",
                Some(&format!("{count} selected")),
                theme,
            ))
            .child(body);
        let backdrop =
            render_modal_backdrop(cx).child(motion::dialog_in("bulk-remove-dialog-in", card, cx));
        motion::fade_quick("bulk-remove-dialog-backdrop-in", backdrop, cx).into_any_element()
    }
}

/// A labeled `TextInput` field: a small muted label above the field itself.
/// SURFACES §7: "Fields: label at `TEXT_SM`/`text_muted` above an inset well
/// at `RADIUS_CONTROL`" — the label half of that is this function's whole
/// job; the well itself is `TextInput`'s own paint (`crate::text_input`,
/// out of this task's file scope). Returns a concrete `Div`, not `impl
/// IntoElement`, so callers with a conditional trailing child (the Base
/// field's floating ref picker) can keep chaining `.when(..)`/`.child(..)`
/// on the result.
fn labeled_field(label: &str, input: Entity<TextInput>, theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_4))
        .child(
            div()
                .text_size(px(TEXT_SM))
                .text_color(theme.text_muted)
                .child(label.to_string()),
        )
        .child(input)
}

/// Small muted placeholder text for a dialog's list area (loading, no
/// matches) — the same shape as [`ui::empty_hint`] but without that
/// helper's `flex_1`, which would fight the fixed `max_h` scroll areas
/// dialogs use it inside.
fn dialog_hint(text: &str, theme: &Theme) -> impl IntoElement {
    div()
        .py(px(SPACE_6))
        .text_size(px(TEXT_XS))
        .text_color(theme.text_ghost)
        .child(text.to_string())
}

fn prune_empty_hint(state: &PruneState, theme: &Theme) -> impl IntoElement {
    let text = if state.merged || state.gone {
        "Nothing matches the current filters."
    } else {
        "Only missing or already-prunable worktrees are swept by default — turn on \
         Merged or Upstream gone to include more."
    };
    div()
        .py(px(SPACE_8))
        .text_size(px(TEXT_SM))
        .text_color(theme.text_faint)
        .child(text)
}

/// States, in words, how many worktrees a destructive action affects —
/// SURFACES §7: "the remove/prune dialogs are the destructive ones... the
/// count is stated in words above the action bar." `verb` is the plain
/// infinitive ("prune", "remove"). Shared by the Prune and bulk-remove
/// confirmations, the two dialogs that can ever act on more than one
/// worktree at once; the single-target Remove dialog already names its one
/// worktree in the modal header, so it has no need of this.
fn destructive_count_line(count: usize, verb: &str, theme: &Theme) -> impl IntoElement {
    div()
        .text_size(px(TEXT_SM))
        .text_color(theme.text_muted)
        .child(format!(
            "This will {verb} {count} worktree{}.",
            if count == 1 { "" } else { "s" }
        ))
}

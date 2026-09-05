//! State, pure logic, and rendering for the "Run Command" dialog: a small
//! form to run an arbitrary shell command inside one worktree, then a live
//! view streaming its output.
//!
//! Split the same way [`crate::dialogs`] splits from [`crate::app`], except
//! the *interactive* rendering (click handlers, `cx.listener`) lives here
//! too, since it has to call back into the very actions it exposes (Run,
//! Cancel, pick a recent command) — see [`crate::app::dialog_actions`] for
//! the `Context<WtmApp>`-only wiring (opening the dialog, submitting the
//! form, draining the streaming channel).
//!
//! Closing this dialog while a command is still running does **not** kill
//! it: `data::run_command_streaming` runs to completion on its own
//! background thread regardless of whether anything is still listening for
//! its output. The dialog's own footer says as much while a command is in
//! flight (see `render_progress_footer`).

use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, Context, Entity, ScrollHandle, SharedString, Subscription, Window,
};

use wtm::model::WorktreeInfo;

use crate::app::WtmApp;
use crate::assets::icons;
use crate::data;
use crate::dialogs;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::{Theme, RADIUS_CONTROL, SPACE_12, SPACE_16, SPACE_2, SPACE_4, SPACE_6, SPACE_8};
use crate::ui::{self, ButtonVariant, TEXT_BASE, TEXT_SM, TEXT_XS};

/// Cap on retained output lines. Past this, further lines are counted in
/// [`RunProgressState::dropped`] rather than pushed — output stays bounded
/// in memory instead of growing without limit, and the view says honestly
/// how many lines were left out instead of silently truncating.
const MAX_OUTPUT_LINES: usize = 4000;

/// How many recent-command suggestions the picker shows beneath the input.
pub const MAX_RECENT_SUGGESTIONS: usize = 8;

/// How many recent commands are remembered per repository, session-only —
/// see [`crate::app::WtmApp`]'s `recent_commands` field doc for why this
/// cannot yet survive a restart.
pub const MAX_RECENT_STORED: usize = 20;

const WIDTH: f32 = 480.0;

// ---------------------------------------------------------------------
// State
// ---------------------------------------------------------------------

/// State for the Run Command dialog: which worktree it targets, the command
/// field, and (once submitted) the streaming output view.
pub struct RunCommandState {
    /// Snapshot of the worktree this run targets, taken when the dialog
    /// opened — the same "clone, don't index" reasoning `dialogs::RemoveState`
    /// already uses, so a background reload landing while this dialog is
    /// open can never retarget which worktree Enter actually runs in.
    pub target: WorktreeInfo,
    pub command_input: Entity<TextInput>,
    // Held only to keep the subscription alive — see `dialogs::CreateState`
    // for the same convention.
    _input_sub: Subscription,
    pub phase: RunPhase,
}

/// The dialog has exactly two phases: filling out the command, and watching
/// it run. There is no going back to `Form` once a run is in flight.
pub enum RunPhase {
    Form,
    Running(RunProgressState),
}

/// The streaming output view: the command that was submitted, its captured
/// output (capped — see [`MAX_OUTPUT_LINES`]), and the outcome once the run
/// finishes (`None` while still in flight).
pub struct RunProgressState {
    pub command: String,
    pub log: Vec<String>,
    /// How many output lines arrived beyond [`MAX_OUTPUT_LINES`] and were
    /// therefore not retained.
    pub dropped: usize,
    pub scroll: ScrollHandle,
    pub outcome: Option<RunOutcome>,
}

/// How a submitted command ended up, once it's known.
pub enum RunOutcome {
    /// The command ran to completion. A non-zero `code` is a normal,
    /// ordinary outcome to display here — not an error — mirroring
    /// `data::CommandEvent::Finished`'s own doc comment on why a failing
    /// command is not folded into an `Err`.
    Finished { success: bool, code: Option<i32> },
    /// The command could never be started at all (e.g. `sh` itself is
    /// missing) — `data::run_command_streaming`'s one real `Err` case.
    StartFailed(String),
}

impl RunProgressState {
    fn new(command: String) -> Self {
        Self {
            command,
            log: Vec::new(),
            dropped: 0,
            scroll: ScrollHandle::new(),
            outcome: None,
        }
    }

    /// Append one output line, capped at [`MAX_OUTPUT_LINES`] retained
    /// lines — past that, further lines only increment `dropped`. Keeps the
    /// scroll position pinned to the newest line.
    pub fn push_line(&mut self, line: String) {
        if self.log.len() < MAX_OUTPUT_LINES {
            self.log.push(line);
        } else {
            self.dropped += 1;
        }
        self.scroll.scroll_to_item(self.log.len().saturating_sub(1));
    }
}

/// What crosses from the background `run_command_streaming` task to the
/// foreground drain loop — mirrors `dialogs::StreamMsg`, including bundling
/// the terminal `Result` into the same channel as the streamed events (see
/// that type's doc comment for why a second, separate "are we done" signal
/// would be riskier).
pub enum RunStreamMsg {
    Event(data::CommandEvent),
    Done(Result<(), String>),
}

impl RunCommandState {
    /// Build the dialog's command field and wire its `Submit`/`Cancel`/
    /// `Changed` events straight to `WtmApp` — the one place in this module
    /// that needs `Context<WtmApp>` for state construction, the same split
    /// `dialogs::CreateState::new` uses.
    pub fn new(target: WorktreeInfo, window: &mut Window, cx: &mut Context<WtmApp>) -> Self {
        let command_input = cx.new(|cx| TextInput::new("command to run, e.g. npm test", cx));
        let sub = cx.subscribe_in(&command_input, window, {
            move |app: &mut WtmApp, _input, event, window, cx| match event {
                InputEvent::Submit => app.submit_run_command(window, cx),
                InputEvent::Cancel => app.close_dialog(window, cx),
                InputEvent::Changed => cx.notify(),
            }
        });
        Self {
            target,
            command_input,
            _input_sub: sub,
            phase: RunPhase::Form,
        }
    }

    /// Enter the running phase for `command`. Called once, when the form is
    /// submitted; there is no way back to `Form` from here.
    pub fn start_running(&mut self, command: String) {
        self.phase = RunPhase::Running(RunProgressState::new(command));
    }
}

// ---------------------------------------------------------------------
// Pure logic — unit tested below
// ---------------------------------------------------------------------

/// Commands from `recent` (most-recently-run first) whose text contains
/// `query` as a case-insensitive substring, capped at
/// [`MAX_RECENT_SUGGESTIONS`] — see [`dialogs::substring_filter`]. An empty
/// (or all-whitespace) query matches everything, so the suggestion list
/// shows recent history until the user starts narrowing it.
pub fn filter_recent<'a>(recent: &'a [String], query: &str) -> Vec<&'a str> {
    dialogs::substring_filter(recent, query, String::as_str)
        .into_iter()
        .map(String::as_str)
        .take(MAX_RECENT_SUGGESTIONS)
        .collect()
}

/// Record `command` as just-run: if it already appears in `recent`, move it
/// to the front (a re-run reads as "most recent", not duplicated);
/// otherwise insert it at the front. Either way `recent` is then truncated
/// to `cap` entries, so the oldest falls off first — a plain LRU list.
pub fn record_recent_command(recent: &mut Vec<String>, command: String, cap: usize) {
    recent.retain(|c| c != &command);
    recent.insert(0, command);
    recent.truncate(cap);
}

// ---------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------

/// Render the Run Command dialog. `recent` is this repository's recent
/// commands (see `WtmApp::recent_commands`), handed in rather than read off
/// a field this module doesn't own — the same shape `palette::render` takes
/// `rows`.
pub fn render(
    state: &RunCommandState,
    recent: &[String],
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> AnyElement {
    let body: AnyElement = match &state.phase {
        RunPhase::Form => render_form(state, recent, theme, cx).into_any_element(),
        RunPhase::Running(progress) => render_progress(progress, theme, cx).into_any_element(),
    };

    let card = ui::modal_card(WIDTH, theme)
        .id("run-command-dialog-card")
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(ui::modal_header(
            "Run Command",
            Some(&format!("in {}", state.target.display_name())),
            theme,
        ))
        .child(
            // The target worktree named plainly, in full, right
            // under the header — so which worktree this runs in is
            // never in doubt, even if `display_name()` alone (just
            // the branch) is ambiguous across two worktrees with
            // the same branch name in different repos, or simply
            // easy to skim past.
            div().px(px(SPACE_16)).pb(px(SPACE_2)).child(ui::meta(
                icons::FOLDER,
                state.target.path.display().to_string(),
                theme,
            )),
        )
        .child(body);

    crate::app::present_modal("run-command-dialog", card, cx)
}

fn render_form(
    state: &RunCommandState,
    recent: &[String],
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> impl IntoElement {
    let query = state.command_input.read(cx).value().to_string();
    let suggestions = filter_recent(recent, &query);
    let can_submit = !query.trim().is_empty();

    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_12))
        .px(px(SPACE_16))
        .py(px(SPACE_12))
        .child(
            // Field label at `TEXT_SM`/`text_muted` above the input well.
            div()
                .flex()
                .flex_col()
                .gap(px(SPACE_4))
                .child(
                    div()
                        .text_size(px(TEXT_SM))
                        .text_color(theme.text_muted)
                        .child("Command"),
                )
                .child(state.command_input.clone()),
        )
        .when(!suggestions.is_empty(), |this| {
            // No "Recent" eyebrow label: the outer form's own `SPACE_12`
            // gap above this list (more than 2x this list's own `SPACE_2`
            // row gap, `better-layout` §1) already reads as a new group
            // below the Command field, and this is a "Run Command" dialog,
            // so a list of plain command strings under the command field is
            // unambiguous without a label.
            this.child(
                div()
                    .id("run-command-recent")
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_2))
                    .max_h(px(150.0))
                    .overflow_y_scroll()
                    .children(suggestions.into_iter().map(|command| {
                        let picked = command.to_string();
                        ui::row(
                            SharedString::from(format!("recent-command-{command}")),
                            false,
                            theme,
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(TEXT_BASE))
                                .text_color(theme.text)
                                .child(command.to_string()),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.select_recent_command(picked.clone(), window, cx);
                            },
                        ))
                    })),
            )
        })
        .child(
            ui::modal_footer(theme)
                .child(
                    ui::button(
                        "run-command-cancel",
                        "Cancel",
                        ButtonVariant::Secondary,
                        theme,
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
                )
                .child({
                    let button =
                        ui::button("run-command-run", "Run", ButtonVariant::Primary, theme);
                    if can_submit {
                        button
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_run_command(window, cx)
                            }))
                            .into_any_element()
                    } else {
                        ui::disabled(button.opacity(0.4)).into_any_element()
                    }
                }),
        )
}

fn render_progress(
    progress: &RunProgressState,
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_12))
        .px(px(SPACE_16))
        .py(px(SPACE_12))
        .child(
            div()
                .text_size(px(TEXT_BASE))
                .text_color(theme.text)
                .font(ui::mono_font())
                .child(format!("$ {}", progress.command)),
        )
        .child(
            // Mono on `surface_inset`, newest line pinned in view
            // (`track_scroll` + each push scrolling to the last item — see
            // `RunProgressState::push_line`).
            div()
                .id("run-command-log")
                .flex()
                .flex_col()
                .h(px(240.0))
                .overflow_y_scroll()
                .track_scroll(&progress.scroll)
                .px(px(SPACE_8))
                .py(px(SPACE_8))
                .rounded(px(RADIUS_CONTROL))
                .bg(theme.surface_inset)
                .font(ui::mono_font())
                .children(
                    progress
                        .log
                        .iter()
                        .map(|line| render_output_line(line, theme)),
                )
                .when(progress.dropped > 0, |this| {
                    this.child(
                        div()
                            .pt(px(SPACE_4))
                            .text_size(px(TEXT_XS))
                            .text_color(theme.text_ghost)
                            .child(format!(
                                "…{} more line{} not shown (output capped at {} lines)",
                                progress.dropped,
                                if progress.dropped == 1 { "" } else { "s" },
                                MAX_OUTPUT_LINES,
                            )),
                    )
                }),
        )
        .when_some(progress.outcome.as_ref(), |this, outcome| {
            this.child(render_outcome_banner(outcome, theme))
        })
        .child(render_progress_footer(progress, theme, cx))
}

fn render_output_line(text: &str, theme: &Theme) -> impl IntoElement {
    div()
        .text_size(px(TEXT_XS))
        .line_height(px(16.0))
        .text_color(theme.text_muted)
        .child(ui::non_empty_or_space(text).to_string())
}

/// The finished/failed banner: `theme.success` for a clean exit,
/// `theme.danger` for anything else — a non-zero exit is shown here exactly
/// like a clean one, just tinted differently, never as an error dialog.
/// Color is paired with both an icon and the outcome spelled out in the
/// label text — never a tinted word alone.
fn render_outcome_banner(outcome: &RunOutcome, theme: &Theme) -> impl IntoElement {
    let (color, icon_path, text) = match outcome {
        RunOutcome::Finished {
            success: true,
            code: _,
        } => (
            theme.success,
            crate::assets::icons::CHECK,
            "Exited 0".to_string(),
        ),
        RunOutcome::Finished {
            success: false,
            code: Some(code),
        } => (
            theme.danger,
            crate::assets::icons::CIRCLE_ALERT,
            format!("Exited {code}"),
        ),
        RunOutcome::Finished {
            success: false,
            code: None,
        } => (
            theme.danger,
            crate::assets::icons::CIRCLE_ALERT,
            "Terminated by signal".to_string(),
        ),
        RunOutcome::StartFailed(e) => (
            theme.danger,
            crate::assets::icons::CIRCLE_ALERT,
            format!("Could not start: {e}"),
        ),
    };
    div()
        .flex()
        .items_center()
        .gap(px(SPACE_6))
        .text_size(px(TEXT_BASE))
        .text_color(color)
        .child(ui::icon(icon_path, 12.0, color))
        .child(text)
}

fn render_progress_footer(
    progress: &RunProgressState,
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> impl IntoElement {
    let footer = ui::modal_footer(theme);
    let footer = if progress.outcome.is_none() {
        footer.child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(TEXT_XS))
                .text_color(theme.text_ghost)
                .child("Running… closing this dialog will not stop it."),
        )
    } else {
        footer
    };
    footer.child(
        ui::button(
            "run-command-close",
            "Close",
            ButtonVariant::Secondary,
            theme,
        )
        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------- RunProgressState::push_line ----------------

    #[test]
    fn push_line_collects_under_the_cap() {
        let mut state = RunProgressState::new("test".to_string());
        for i in 0..5 {
            state.push_line(format!("line {i}"));
        }
        assert_eq!(state.log.len(), 5);
        assert_eq!(state.dropped, 0);
    }

    #[test]
    fn push_line_caps_and_counts_the_rest() {
        let mut state = RunProgressState::new("test".to_string());
        for i in 0..MAX_OUTPUT_LINES + 7 {
            state.push_line(format!("line {i}"));
        }
        assert_eq!(
            state.log.len(),
            MAX_OUTPUT_LINES,
            "retained lines never exceed the cap"
        );
        assert_eq!(
            state.dropped, 7,
            "every line past the cap is counted, not silently lost"
        );
    }

    // ---------------- recent-command filter/recorder ----------------

    #[test]
    fn filter_recent_empty_query_returns_everything_in_order() {
        let recent = vec!["npm test".to_string(), "cargo build".to_string()];
        let filtered = filter_recent(&recent, "");
        assert_eq!(filtered, vec!["npm test", "cargo build"]);
    }

    #[test]
    fn filter_recent_matches_case_insensitive_substring() {
        let recent = vec!["npm test".to_string(), "cargo build".to_string()];
        let filtered = filter_recent(&recent, "TEST");
        assert_eq!(filtered, vec!["npm test"]);
    }

    #[test]
    fn filter_recent_no_match_is_empty() {
        let recent = vec!["npm test".to_string()];
        assert!(filter_recent(&recent, "nonexistent").is_empty());
    }

    #[test]
    fn filter_recent_caps_suggestions() {
        let recent: Vec<String> = (0..20).map(|i| format!("command {i}")).collect();
        let filtered = filter_recent(&recent, "");
        assert_eq!(filtered.len(), MAX_RECENT_SUGGESTIONS);
    }

    #[test]
    fn record_recent_command_inserts_new_commands_at_the_front() {
        let mut recent = vec!["b".to_string()];
        record_recent_command(&mut recent, "a".to_string(), 10);
        assert_eq!(recent, vec!["a", "b"]);
    }

    #[test]
    fn record_recent_command_moves_a_repeat_to_the_front_without_duplicating() {
        let mut recent = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        record_recent_command(&mut recent, "b".to_string(), 10);
        assert_eq!(recent, vec!["b", "a", "c"]);
    }

    #[test]
    fn record_recent_command_caps_and_evicts_the_oldest() {
        let mut recent = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        record_recent_command(&mut recent, "d".to_string(), 3);
        assert_eq!(recent, vec!["d", "a", "b"], "c (the oldest) falls off");
    }
}

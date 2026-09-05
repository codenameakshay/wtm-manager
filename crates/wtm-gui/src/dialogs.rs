//! State and pure logic for the three modal dialogs: create, remove, and
//! prune. At most one is ever open, which is why [`Dialog`] is an enum
//! rather than three independent `Option`s — the alternative lets two
//! dialogs exist "open" at once in state even though the UI could only ever
//! show one, which is exactly the kind of representable-but-impossible state
//! this module exists to rule out.
//!
//! What lives here is the model: what each dialog knows, and the logic that
//! doesn't need a window to run (branch filtering, the remove
//! confirm-button predicate, prune candidate recomputation). Rendering with
//! click handlers, background spawns, and everything else that needs
//! `Context<WtmApp>` stays in [`crate::app`], the same split
//! [`crate::worktree_list`] uses for the main list: this module hands back
//! plain data and presentational pieces, `app` wires them to actions.
//!
//! [`CreateState`] is the one exception that reaches into `app`: its
//! [`CreateState::new`] wires up [`crate::text_input::TextInput`]
//! subscriptions, which only make sense in terms of the view that owns
//! them.

use std::path::PathBuf;

use gpui::prelude::*;
use gpui::{
    div, px, Context, Div, Entity, Focusable, Hsla, ScrollHandle, SharedString, Stateful,
    Subscription,
};

use wtm::commands::prune::PruneCandidate;
use wtm::model::WorktreeInfo;
use wtm::setup::SetupEvent;

use crate::app::WtmApp;
use crate::data::{BranchInfo, OpenRepo, RefInfo, RefKind};
use crate::text_input::{InputEvent, TextInput};
use crate::theme::{Theme, RADIUS_CHIP, RADIUS_ROW, SPACE_12, SPACE_2, SPACE_4, SPACE_6, SPACE_8};
use crate::ui::{self, TEXT_BASE, TEXT_XS};

/// The one dialog that may be open at a time.
pub enum Dialog {
    Create(CreateState),
    Remove(RemoveState),
    Prune(PruneState),
}

// ---------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------

/// State for the create-worktree dialog: the form fields, the branch picker
/// beneath them, and (once submitted) the streaming progress view.
pub struct CreateState {
    pub branch_input: Entity<TextInput>,
    pub base_input: Entity<TextInput>,
    // Held only to keep the subscriptions alive — see the module doc on
    // [`InputEvent`]: an unheld `Subscription` is dropped immediately and
    // stops firing.
    _branch_sub: Subscription,
    _base_sub: Subscription,
    // Opens/closes the base-ref picker as `base_input` gains/loses focus —
    // see `CreateState::new`'s doc comment on why focus, not a dedicated
    // button, drives it.
    _base_focus_sub: Subscription,
    _base_blur_sub: Subscription,
    /// Branches loaded from `list_branches`, for the filtered picker below
    /// the branch field. Empty until the background load finishes.
    ///
    /// Deliberately still `list_branches`, not `list_refs`: this field names
    /// a *new* branch, so a local branch already checked out elsewhere needs
    /// exactly the disabled "checked out" treatment `list_branches`/
    /// `render_branch_row` already give it (`wtm add` would refuse it) — and
    /// this field has no use for `list_refs`' remote-tracking entries or its
    /// `Current`/`Default` synthetic rows, none of which name something you
    /// could create a *new* branch called.
    pub branches: Vec<BranchInfo>,
    pub branches_loading: bool,
    /// Refs offered by the Base field's picker: local branches, remote-
    /// tracking branches, and the synthetic `Current`/`Default` entries —
    /// see `crate::data::list_refs`. Unlike `branches` above, a branch
    /// checked out in another worktree is a perfectly good *base* to branch
    /// from (only the *new* branch name can't collide with one already
    /// checked out), so nothing here is ever disabled. Empty until the
    /// background load finishes.
    pub base_refs: Vec<RefInfo>,
    pub base_refs_loading: bool,
    /// Whether the Base field's floating ref picker is currently shown.
    /// Opened by focusing `base_input` (click or Tab), closed by Escape, by
    /// picking a row, or by the field losing focus — see
    /// `WtmApp::open_base_picker`/`close_base_picker`.
    pub base_picker_open: bool,
    /// Keyboard highlight into the picker's *filtered* ref list. Not kept in
    /// range on every keystroke — `clamp_highlight` resolves it against the
    /// current filtered length wherever it's read, the same convention
    /// `PaletteState::highlighted` uses for the same reason (the filtered
    /// list, and therefore what counts as "in range", changes on every
    /// keystroke).
    pub base_picker_highlight: usize,
    pub run_setup: bool,
    /// Whether the repo has any setup commands or copy entries at all — the
    /// toggle is disabled and explained rather than hidden when this is
    /// false, so the user learns *why* nothing runs instead of wondering
    /// where the option went.
    pub setup_available: bool,
    pub phase: CreatePhase,
}

/// The create dialog has exactly two phases: filling out the form, and
/// watching it run. There is no going back from `Progress` to `Form` — once
/// a create is in flight there is nothing left to edit.
pub enum CreatePhase {
    Form,
    Progress(ProgressState),
}

/// Progress view state: the streamed setup log, plus the outcome once the
/// background create finishes (`None` while still running).
pub struct ProgressState {
    pub branch: String,
    /// Filled in once creation succeeds; `None` beforehand and on failure.
    pub destination: Option<PathBuf>,
    pub log: Vec<LogEntry>,
    pub scroll: ScrollHandle,
    pub outcome: Option<Result<PathBuf, String>>,
}

impl ProgressState {
    fn new(branch: String) -> Self {
        Self {
            branch,
            destination: None,
            log: Vec::new(),
            scroll: ScrollHandle::new(),
            outcome: None,
        }
    }

    /// Turn one streamed setup step into a log line. Command output and
    /// copy bookkeeping read as quiet, secondary text; a failed command is
    /// the one line that needs to stand out, since it's the one line that
    /// explains why the worktree is left half set up.
    pub fn push_event(&mut self, event: SetupEvent) {
        match event {
            SetupEvent::CopyStarted { path } => self.push(LogKind::Info, format!("copying {path}")),
            SetupEvent::CopyFinished { path } => self.push(LogKind::Info, format!("copied {path}")),
            SetupEvent::CommandStarted { command } => {
                self.push(LogKind::Info, format!("$ {command}"))
            }
            SetupEvent::CommandOutput { line } => self.push(LogKind::Output, line),
            SetupEvent::CommandFinished { command, success } => {
                if !success {
                    self.push(LogKind::Error, format!("`{command}` failed"));
                }
            }
        }
    }

    pub fn push_error(&mut self, message: String) {
        self.push(LogKind::Error, message);
    }

    fn push(&mut self, kind: LogKind, text: String) {
        self.log.push(LogEntry { kind, text });
        self.scroll.scroll_to_item(self.log.len().saturating_sub(1));
    }
}

pub struct LogEntry {
    pub kind: LogKind,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Info,
    Output,
    Error,
}

/// What crosses from the background create task to the foreground drain
/// loop. Bundling the terminal result into the same channel as the setup
/// events means the drain loop only has to watch one thing to know both
/// "what happened" and "are we done" — see `WtmApp::submit_create_dialog`
/// for why a second, separate "are we done" signal would risk the drain
/// loop never noticing completion.
pub enum StreamMsg {
    Event(SetupEvent),
    Done(Result<PathBuf, String>),
}

impl CreateState {
    /// Build the dialog's two text fields and wire their `Submit`/`Cancel`/
    /// `Changed` events straight to the owning `WtmApp` — this is the one
    /// place in this module that needs `Context<WtmApp>` rather than plain
    /// data, because a `Subscription` is only meaningful in terms of the
    /// entity that outlives it.
    pub fn new(repo: &OpenRepo, window: &mut gpui::Window, cx: &mut Context<WtmApp>) -> Self {
        let base_placeholder = repo
            .config
            .default_base
            .clone()
            .unwrap_or_else(|| "HEAD".to_string());
        let branch_input = cx.new(|cx| TextInput::new("branch name", cx));
        let base_input = cx.new(|cx| TextInput::new(base_placeholder, cx));

        // `subscribe_in` rather than `subscribe`: closing the dialog or
        // submitting the form both need to hand focus back to the root
        // explicitly (see `WtmApp::close_dialog`/`submit_create_dialog`),
        // which takes a `&mut Window` that plain `subscribe`'s callback
        // never receives.
        let branch_sub = cx.subscribe_in(
            &branch_input,
            window,
            |app: &mut WtmApp, _input, event, window, cx| match event {
                InputEvent::Submit => app.submit_create_dialog(window, cx),
                InputEvent::Cancel => app.close_dialog(window, cx),
                // The branch field's value drives the filtered picker below
                // it, which is computed at render time — so a keystroke has
                // to force a repaint or the list would only ever update on
                // some unrelated event.
                InputEvent::Changed => cx.notify(),
            },
        );
        // Unlike `branch_sub`, `InputEvent::Submit`/`Cancel` here route
        // through the picker first (`submit_create_or_pick_base`/
        // `close_base_picker_or_dialog`) rather than straight to
        // `submit_create_dialog`/`close_dialog` — see those methods' doc
        // comments for why Enter/Escape mean something different while the
        // picker is open. `Changed` still just repaints: a keystroke
        // re-filters whichever of the picker's list or the "no matches"
        // hint is showing, computed fresh at render time from `base_refs`.
        let base_sub = cx.subscribe_in(
            &base_input,
            window,
            |app: &mut WtmApp, _input, event, window, cx| match event {
                InputEvent::Submit => app.submit_create_or_pick_base(window, cx),
                InputEvent::Cancel => app.close_base_picker_or_dialog(window, cx),
                InputEvent::Changed => cx.notify(),
            },
        );

        // The picker opens and closes with `base_input`'s own focus — a
        // click or Tab into the field shows suggestions, moving focus
        // elsewhere (to the branch field, say) hides them again — rather
        // than a dedicated toggle button: every dialog field here already
        // grabs focus on click for free via `TextInput`'s `track_focus`,
        // so focus is the one signal already flowing through this exact
        // field with no new wiring needed. Escape
        // (`close_base_picker_or_dialog` above) closes the picker
        // *without* blurring the field, so typing can continue right
        // after — see that method's doc comment.
        let base_focus_handle = base_input.focus_handle(cx);
        let base_focus_sub = cx.on_focus(
            &base_focus_handle,
            window,
            |app: &mut WtmApp, _window, cx| {
                app.open_base_picker(cx);
            },
        );
        let base_blur_sub = cx.on_blur(
            &base_focus_handle,
            window,
            |app: &mut WtmApp, _window, cx| {
                app.close_base_picker(cx);
            },
        );

        let setup_available =
            !repo.config.setup.commands.is_empty() || !repo.config.setup.copy.is_empty();

        Self {
            branch_input,
            base_input,
            _branch_sub: branch_sub,
            _base_sub: base_sub,
            _base_focus_sub: base_focus_sub,
            _base_blur_sub: base_blur_sub,
            branches: Vec::new(),
            branches_loading: true,
            base_refs: Vec::new(),
            base_refs_loading: true,
            base_picker_open: false,
            base_picker_highlight: 0,
            run_setup: setup_available,
            setup_available,
            phase: CreatePhase::Form,
        }
    }

    /// Enter the progress phase for `branch`. Called once, when the form is
    /// submitted; there is no way back to `Form` from here.
    pub fn start_progress(&mut self, branch: String) {
        self.phase = CreatePhase::Progress(ProgressState::new(branch));
    }
}

/// Case-insensitive substring filter shared by the create dialog's branch
/// picker and the run-command dialog's recent-command list. An empty (or
/// all-whitespace) query matches everything, preserving `items`' own order
/// — the picker shows the full list until the user starts narrowing it,
/// rather than an empty list waiting for input.
///
/// Branch and command names are ASCII in every real case, so this matches
/// byte-for-byte ignoring ASCII case rather than lowercasing each item —
/// `query` is trimmed once up front instead of on every comparison.
pub(crate) fn substring_filter<'a, T>(
    items: &'a [T],
    query: &str,
    key: impl Fn(&T) -> &str,
) -> Vec<&'a T> {
    let query = query.trim();
    if query.is_empty() {
        return items.iter().collect();
    }
    let query = query.as_bytes();
    items
        .iter()
        .filter(|item| {
            key(item)
                .as_bytes()
                .windows(query.len())
                .any(|w| w.eq_ignore_ascii_case(query))
        })
        .collect()
}

/// Branches matching `query` as a case-insensitive substring of the branch
/// name — see [`substring_filter`].
pub fn filter_branches<'a>(branches: &'a [BranchInfo], query: &str) -> Vec<&'a BranchInfo> {
    substring_filter(branches, query, |b| b.name.as_str())
}

/// One row in the branch picker: name, plus a "checked out" hint (disabled,
/// per `wtm add`'s `BranchInUse` refusal) or a "gone" pill for a local
/// branch whose upstream disappeared. Purely presentational — the caller
/// decides whether to attach a click handler based on `branch.is_checked_out`.
pub fn render_branch_row(branch: &BranchInfo, theme: &Theme) -> Stateful<Div> {
    let disabled = branch.is_checked_out;

    ui::row(
        SharedString::from(format!("branch-{}", branch.name)),
        false,
        theme,
    )
    .flex()
    .items_center()
    .justify_between()
    .gap(px(SPACE_8))
    .child(
        div()
            .min_w_0()
            .truncate()
            .text_size(px(TEXT_BASE))
            .text_color(if disabled {
                theme.text_ghost
            } else {
                theme.text
            })
            .child(branch.name.clone()),
    )
    .when(disabled, |this| {
        this.child(
            div()
                .flex_none()
                .text_size(px(TEXT_XS))
                .text_color(theme.text_ghost)
                .child("checked out"),
        )
    })
    .when(!disabled && branch.upstream_gone, |this| {
        this.child(ui::pill("gone", theme.danger))
    })
}

/// A log line, tinted by what kind of setup step it reports: quiet info for
/// bookkeeping (copy/command start), quieter still for the command's own
/// output, and `theme.danger` for the one line that means setup didn't
/// finish clean. Every line takes [`ui::mono_font`] regardless of kind — the
/// whole log reads as one console, not a mix of proportional and monospace
/// text.
pub fn render_log_entry(entry: &LogEntry, theme: &Theme) -> impl IntoElement {
    let color = match entry.kind {
        LogKind::Info => theme.text_faint,
        LogKind::Output => theme.text_ghost,
        LogKind::Error => theme.danger,
    };
    div()
        .font(ui::mono_font())
        .text_size(px(TEXT_XS))
        .line_height(px(16.0))
        .text_color(color)
        .child(entry.text.clone())
}

/// A labeled toggle row: a checkbox glyph plus its label, dimmed and
/// non-interactive-looking when `disabled`. Shared by every toggle across
/// all three dialogs (run setup, force, delete branch, merged, gone) and
/// the settings sheet's "Reduce motion" toggle, so they read as one
/// control, not five reinvented ones.
pub fn render_toggle(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    checked: bool,
    disabled: bool,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .items_center()
        .gap(px(SPACE_8))
        .py(px(SPACE_4))
        .cursor_default()
        .child(
            div()
                .w(px(14.0))
                .h(px(14.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(RADIUS_CHIP))
                .border_1()
                .border_color(if disabled {
                    theme.border
                } else if checked {
                    theme.accent
                } else {
                    theme.border_strong
                })
                .bg(if checked && !disabled {
                    theme.accent
                } else {
                    gpui::transparent_black()
                })
                .when(checked, |this| {
                    this.child(ui::icon(
                        crate::assets::icons::CHECK,
                        10.0,
                        if disabled {
                            theme.text_ghost
                        } else {
                            // The checkmark sits on an `accent`-filled plate
                            // — `on_accent` is the token built for exactly
                            // that, never `theme.text`/`bg` on a colored
                            // plate.
                            theme.on_accent
                        },
                    ))
                }),
        )
        .child(
            div()
                .text_size(px(TEXT_BASE))
                .text_color(if disabled {
                    theme.text_ghost
                } else {
                    theme.text
                })
                .child(label.into()),
        )
}

// ---------------------------------------------------------------------
// Base-ref picker (Base field, create dialog)
// ---------------------------------------------------------------------

/// Refs matching `query`, ranked by `crate::palette::fuzzy_match`'s score
/// (best first) — the same fuzzy scorer the command palette uses, reused
/// rather than reimplemented since `fuzzy_match` is a public function of
/// this crate. Matching is against `name`, the same field the row renders.
/// `sort_by_key` is stable, so an empty query — every ref scores `0` —
/// leaves `refs`' own order untouched: `list_refs`' Current, Default,
/// locals-then-remotes ordering is exactly what a picker should browse
/// before the user has typed anything to rank by.
pub fn filter_refs<'a>(refs: &'a [RefInfo], query: &str) -> Vec<&'a RefInfo> {
    let mut scored: Vec<(i64, &RefInfo)> = refs
        .iter()
        .filter_map(|r| {
            let m = crate::palette::fuzzy_match(query, &r.name)?;
            Some((m.score, r))
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.into_iter().map(|(_, r)| r).collect()
}

/// Resolve a stored highlight index against the *current* result count:
/// `0` when there is nothing to highlight, otherwise clamped to the last
/// valid index. Used both to decide which row paints as highlighted and,
/// when Enter is pressed, which ref it actually picks — sharing this one
/// function is what keeps those two agreeing after a keystroke shrinks the
/// filtered list out from under a highlight that pointed further down.
pub fn clamp_highlight(highlighted: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        highlighted.min(len - 1)
    }
}

/// Move the picker's highlight by `delta` (`1` for Down, `-1` for Up),
/// wrapping at either end — the same wraparound
/// `WtmApp::palette_move_highlight` already uses for the command palette,
/// so every fuzzy list in this app agrees on what Up from the top (or Down
/// from the bottom) does. `0` when there is nothing to highlight.
pub(crate) fn move_highlight(highlighted: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let clamped = clamp_highlight(highlighted, len) as i32;
    (clamped + delta).rem_euclid(len as i32) as usize
}

/// The muted tag shown at a ref row's right edge, naming what the ref is —
/// `None` for a plain local branch, which is the common, unremarkable case
/// and reads better with no badge at all than with a "local" label on every
/// single row.
fn ref_kind_label(kind: &RefKind) -> Option<&'static str> {
    match kind {
        RefKind::Current => Some("current"),
        RefKind::Default => Some("default"),
        RefKind::Worktree => Some("worktree"),
        RefKind::Local => None,
        RefKind::Remote { .. } => Some("remote"),
    }
}

/// One row in the base-ref picker: the ref's name, its kind tag if any (see
/// [`ref_kind_label`]), and — when cheap to get — a second, muted line with
/// its short sha and commit subject, which is what makes "which `main` did
/// I mean" answerable at a glance. `highlighted` paints the same selected
/// wash [`ui::row`] gives a keyboard-highlighted palette entry; the caller
/// attaches `.on_click(...)`, the same split [`render_branch_row`] uses.
///
/// Deliberately has no disabled state, unlike `render_branch_row`: a
/// `RefKind::Worktree` entry is checked out elsewhere, which only matters
/// for the *branch name* field (`wtm add` would refuse to reuse that name)
/// — as a *base* to branch from, it's exactly as valid as any other ref.
pub fn render_ref_row(r: &RefInfo, highlighted: bool, theme: &Theme) -> Stateful<Div> {
    let tag = ref_kind_label(&r.kind);
    let has_meta = r.subject.is_some() || r.short_id.is_some();

    ui::row(
        SharedString::from(format!("ref-{}", r.name)),
        highlighted,
        theme,
    )
    .flex()
    .flex_col()
    .gap(px(SPACE_2))
    .child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(SPACE_8))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(TEXT_BASE))
                    .text_color(theme.text)
                    .child(r.name.clone()),
            )
            .when_some(tag, |this, tag| {
                this.child(
                    div()
                        .flex_none()
                        .text_size(px(TEXT_XS))
                        .text_color(theme.text_faint)
                        .child(tag),
                )
            }),
    )
    .when(has_meta, |this| {
        this.child(
            div()
                .flex()
                .min_w_0()
                .items_baseline()
                .gap(px(SPACE_6))
                .text_size(px(TEXT_XS))
                // The short id is a sha, so it takes the bundled mono face,
                // never the proportional one, so a column of them actually
                // lines up. The subject stays proportional and a step
                // quieter (`text_ghost`), since it's the "what" and the sha
                // is the more scannable "which one".
                .when_some(r.short_id.clone(), |this, id| {
                    this.child(
                        div()
                            .flex_none()
                            .font_family(ui::FONT_MONO)
                            .text_color(theme.text_ghost)
                            .child(id),
                    )
                })
                .when_some(r.subject.clone(), |this, subject| {
                    this.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.text_faint)
                            .child(subject),
                    )
                }),
        )
    })
}

// ---------------------------------------------------------------------
// Remove
// ---------------------------------------------------------------------

/// State for the remove-worktree confirmation.
pub struct RemoveState {
    /// Snapshot of the worktree at the moment the dialog opened. A clone
    /// rather than an index into `WtmApp::rows`: the list can reload out
    /// from under an open dialog (a background refresh, a filesystem
    /// watcher tick), and the dialog should keep describing the worktree
    /// the user actually asked to remove rather than silently retargeting.
    pub target: WorktreeInfo,
    pub force: bool,
    pub delete_branch: bool,
    /// `Some(reason)` when the checked-out branch is in
    /// `prune.protected_branches` — the "delete branch" toggle is disabled
    /// and shows this instead of silently refusing later.
    pub branch_protected: Option<String>,
    pub busy: bool,
    /// Set when a remove attempt fails; shown inline and cleared on retry.
    pub error: Option<String>,
}

impl RemoveState {
    pub fn new(target: WorktreeInfo, protected_branches: &[String]) -> Self {
        let branch_protected = target.branch.as_deref().and_then(|branch| {
            protected_branches
                .iter()
                .any(|p| p == branch)
                .then(|| format!("'{branch}' is a protected branch"))
        });

        Self {
            target,
            force: false,
            delete_branch: false,
            branch_protected,
            busy: false,
            error: None,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.target.status.as_ref().is_some_and(|s| s.dirty)
    }

    /// Whether the destructive button may fire: never for the main
    /// worktree (removing it would leave the repository without one), and
    /// a dirty worktree needs the explicit Force toggle first so
    /// uncommitted changes are never discarded by a stray click.
    pub fn can_confirm(&self) -> bool {
        !self.target.is_main && !self.busy && (!self.is_dirty() || self.force)
    }
}

// ---------------------------------------------------------------------
// Prune
// ---------------------------------------------------------------------

/// State for the prune dialog: the two selection toggles (mirroring `wtm
/// prune --merged --gone`), the force toggle, and the candidate list they
/// produce.
pub struct PruneState {
    pub merged: bool,
    pub gone: bool,
    pub force: bool,
    pub candidates: Vec<PruneCandidate>,
    pub busy: bool,
    /// Candidates dealt with so far while `busy`, for the "n of N" line.
    pub done: usize,
}

impl PruneState {
    pub fn new() -> Self {
        Self {
            merged: false,
            gone: false,
            force: false,
            candidates: Vec::new(),
            busy: false,
            done: 0,
        }
    }

    /// Recompute `candidates` from the current listing. Candidate selection
    /// is pure and cheap (see `wtm::commands::prune::candidates`), so this
    /// runs directly on every toggle change instead of round-tripping
    /// through the background executor.
    pub fn recompute(&mut self, repo: &OpenRepo, rows: &[WorktreeInfo]) {
        self.candidates = crate::data::prune_candidates(repo, rows, self.merged, self.gone);
    }
}

impl Default for PruneState {
    fn default() -> Self {
        Self::new()
    }
}

/// Color a prune reason by what it means: `missing`/`gone` are the reasons
/// that mean "this is unreachable now", `merged` is a calmer "already
/// landed", and anything else (currently just `prunable`) stays neutral.
fn reason_color(reason: &str, theme: &Theme) -> Hsla {
    match reason {
        "missing" | "gone" => theme.danger,
        "merged" => theme.success,
        _ => theme.text_faint,
    }
}

/// One prune candidate: its name, why it was selected, and whether its
/// branch goes with it. Rendered as a proud `surface_raised` plate rather
/// than a plain wash — the affected worktree gets real room, so each row
/// reads as a real, weighty item about to be destroyed, not a throwaway
/// list line.
pub fn render_candidate_row(candidate: &PruneCandidate, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(SPACE_8))
        .px(px(SPACE_12))
        .py(px(SPACE_8))
        .rounded(px(RADIUS_ROW))
        .bg(theme.surface_raised)
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(TEXT_BASE))
                .text_color(theme.text)
                .child(candidate.info.display_name().to_string()),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(SPACE_6))
                .children(
                    candidate
                        .reasons
                        .iter()
                        .map(|reason| ui::pill(*reason, reason_color(reason, theme))),
                )
                .when(candidate.delete_branch, |this| {
                    this.child(
                        div()
                            .text_size(px(TEXT_XS))
                            .text_color(theme.text_ghost)
                            .child("+ branch"),
                    )
                }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use wtm::config::Config;
    use wtm::model::WorktreeStatus;
    use wtm::repo::RepoContext;

    fn branch(name: &str, checked_out: bool) -> BranchInfo {
        BranchInfo {
            name: name.to_string(),
            is_checked_out: checked_out,
            upstream_gone: false,
        }
    }

    fn ref_info(name: &str, kind: RefKind) -> RefInfo {
        RefInfo {
            name: name.to_string(),
            kind,
            subject: None,
            short_id: None,
        }
    }

    // ---------------- Base-ref picker ----------------

    #[test]
    fn filter_refs_empty_query_returns_all_in_list_order() {
        let refs = vec![
            ref_info("main", RefKind::Current),
            ref_info("HEAD", RefKind::Default),
            ref_info("feature", RefKind::Local),
            ref_info(
                "origin/main",
                RefKind::Remote {
                    remote: "origin".to_string(),
                },
            ),
        ];
        let filtered = filter_refs(&refs, "");
        let names: Vec<&str> = filtered.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["main", "HEAD", "feature", "origin/main"]);
    }

    #[test]
    fn filter_refs_matches_fuzzy_subsequence_and_excludes_non_matches() {
        let refs = vec![
            ref_info("feature-login", RefKind::Local),
            ref_info("main", RefKind::Current),
            ref_info(
                "origin/feature-logout",
                RefKind::Remote {
                    remote: "origin".to_string(),
                },
            ),
        ];
        let filtered = filter_refs(&refs, "flogin");
        let names: Vec<&str> = filtered.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["feature-login"]);
    }

    #[test]
    fn filter_refs_ranks_better_matches_first() {
        // "main" matches "main" as a whole-word prefix (all boundary
        // characters) and should outrank "domain", where the same letters
        // are buried mid-word.
        let refs = vec![
            ref_info("domain", RefKind::Local),
            ref_info("main", RefKind::Local),
        ];
        let filtered = filter_refs(&refs, "main");
        let names: Vec<&str> = filtered.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["main", "domain"]);
    }

    #[test]
    fn clamp_highlight_handles_empty_and_out_of_range() {
        assert_eq!(clamp_highlight(0, 0), 0);
        assert_eq!(clamp_highlight(5, 0), 0);
        assert_eq!(clamp_highlight(5, 3), 2);
        assert_eq!(clamp_highlight(1, 3), 1);
    }

    #[test]
    fn move_highlight_steps_within_bounds() {
        assert_eq!(move_highlight(0, 1, 3), 1);
        assert_eq!(move_highlight(1, 1, 3), 2);
        assert_eq!(move_highlight(2, -1, 3), 1);
    }

    #[test]
    fn move_highlight_wraps_at_both_ends() {
        // Down from the last row wraps to the first.
        assert_eq!(move_highlight(2, 1, 3), 0);
        // Up from the first row wraps to the last.
        assert_eq!(move_highlight(0, -1, 3), 2);
    }

    #[test]
    fn move_highlight_with_nothing_to_highlight_stays_zero() {
        assert_eq!(move_highlight(0, 1, 0), 0);
        assert_eq!(move_highlight(4, -1, 0), 0);
    }

    #[test]
    fn move_highlight_clamps_a_stale_index_before_stepping() {
        // A highlight that pointed past the end of a list that just shrank
        // (a keystroke narrowed the results) is clamped, not carried
        // out-of-bounds, before delta is applied.
        assert_eq!(move_highlight(10, 1, 3), 0);
        assert_eq!(move_highlight(10, -1, 3), 1);
    }

    #[test]
    fn ref_kind_label_maps_every_kind() {
        assert_eq!(ref_kind_label(&RefKind::Current), Some("current"));
        assert_eq!(ref_kind_label(&RefKind::Default), Some("default"));
        assert_eq!(ref_kind_label(&RefKind::Worktree), Some("worktree"));
        assert_eq!(ref_kind_label(&RefKind::Local), None);
        assert_eq!(
            ref_kind_label(&RefKind::Remote {
                remote: "origin".to_string()
            }),
            Some("remote")
        );
        // The tag names the *kind*, not which remote — an "upstream" remote
        // reads the same as "origin".
        assert_eq!(
            ref_kind_label(&RefKind::Remote {
                remote: "upstream".to_string()
            }),
            Some("remote")
        );
    }

    #[test]
    fn filter_branches_empty_query_returns_all_in_order() {
        let branches = vec![branch("main", false), branch("feature-x", false)];
        let filtered = filter_branches(&branches, "");
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].name, "main");
        assert_eq!(filtered[1].name, "feature-x");

        // All-whitespace is the same as empty.
        assert_eq!(filter_branches(&branches, "   ").len(), 2);
    }

    #[test]
    fn filter_branches_matches_case_insensitive_substring() {
        let branches = vec![
            branch("feature-Login", false),
            branch("bugfix/LOGIN-crash", false),
            branch("main", false),
        ];
        let filtered = filter_branches(&branches, "login");
        let names: Vec<&str> = filtered.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["feature-Login", "bugfix/LOGIN-crash"]);
    }

    #[test]
    fn filter_branches_no_match_is_empty() {
        let branches = vec![branch("main", false)];
        assert!(filter_branches(&branches, "nonexistent").is_empty());
    }

    fn worktree(name: &str, is_main: bool, dirty: bool) -> WorktreeInfo {
        WorktreeInfo {
            name: name.to_string(),
            path: PathBuf::from(format!("/tmp/{name}")),
            branch: Some(name.to_string()),
            head: None,
            is_main,
            is_missing: false,
            is_locked: false,
            is_prunable: false,
            status: Some(WorktreeStatus {
                dirty,
                dirty_count: usize::from(dirty),
                ahead: None,
                behind: None,
                upstream_gone: false,
                merged: false,
            }),
        }
    }

    #[test]
    fn remove_can_confirm_allows_clean_worktree() {
        let state = RemoveState::new(worktree("feature", false, false), &[]);
        assert!(state.can_confirm());
    }

    #[test]
    fn remove_can_confirm_requires_force_when_dirty() {
        let mut state = RemoveState::new(worktree("feature", false, true), &[]);
        assert!(!state.can_confirm(), "dirty without force must not confirm");
        state.force = true;
        assert!(state.can_confirm(), "dirty with force must confirm");
    }

    #[test]
    fn remove_can_confirm_never_allows_main_worktree() {
        let mut state = RemoveState::new(worktree("main", true, false), &[]);
        assert!(!state.can_confirm());
        state.force = true;
        assert!(
            !state.can_confirm(),
            "force must not unlock the main worktree either"
        );
    }

    #[test]
    fn remove_can_confirm_false_while_busy() {
        let mut state = RemoveState::new(worktree("feature", false, false), &[]);
        state.busy = true;
        assert!(!state.can_confirm());
    }

    fn fake_repo(protected_branches: Vec<String>) -> OpenRepo {
        OpenRepo {
            ctx: RepoContext {
                main_root: PathBuf::from("/tmp/repo"),
                git_dir: PathBuf::from("/tmp/repo/.git"),
                repo_name: "repo".to_string(),
            },
            config: Config {
                prune: wtm::config::PruneConfig { protected_branches },
                ..Config::default()
            },
        }
    }

    #[test]
    fn prune_recompute_starts_empty_with_both_toggles_off() {
        let repo = fake_repo(vec![]);
        let rows = vec![worktree("feature", false, false)];
        let mut state = PruneState::new();
        state.recompute(&repo, &rows);
        assert!(
            state.candidates.is_empty(),
            "neither merged nor gone selected, and nothing missing/prunable"
        );
    }

    #[test]
    fn prune_recompute_reacts_to_merged_toggle() {
        let repo = fake_repo(vec![]);
        let mut merged_row = worktree("feature", false, false);
        merged_row.status = Some(WorktreeStatus {
            dirty: false,
            dirty_count: 0,
            ahead: None,
            behind: None,
            upstream_gone: false,
            merged: true,
        });
        let rows = vec![merged_row];

        let mut state = PruneState::new();
        state.recompute(&repo, &rows);
        assert!(state.candidates.is_empty());

        state.merged = true;
        state.recompute(&repo, &rows);
        assert_eq!(state.candidates.len(), 1);
        assert!(state.candidates[0].reasons.contains(&"merged"));
        assert!(state.candidates[0].delete_branch);
    }

    #[test]
    fn prune_recompute_never_includes_main_or_protected() {
        let repo = fake_repo(vec!["release".to_string()]);
        let mut main_row = worktree("main", true, false);
        main_row.status = Some(WorktreeStatus {
            dirty: false,
            dirty_count: 0,
            ahead: None,
            behind: None,
            upstream_gone: true,
            merged: true,
        });
        let mut protected_row = worktree("release", false, false);
        protected_row.status = main_row.status.clone();
        let rows = vec![main_row, protected_row];

        let mut state = PruneState::new();
        state.merged = true;
        state.gone = true;
        state.recompute(&repo, &rows);
        assert!(state.candidates.is_empty());
    }
}

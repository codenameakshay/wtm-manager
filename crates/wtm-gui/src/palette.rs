//! The command palette (⌘K): one overlay, fuzzy search over both the open
//! repository's worktrees and the app's own actions.
//!
//! Split the same way [`crate::dialogs`] splits from [`crate::app`]: the
//! fuzzy scorer and result ranking ([`fuzzy_match`], [`compute_results`])
//! are pure and heavily unit tested here, while the interactive wiring
//! (subscribing to the search field, dispatching a chosen result) lives on
//! `WtmApp` in `app.rs` — the palette needs `Context<WtmApp>` to call back
//! into the very actions it lists (New Worktree, Reload, …), so unlike
//! `dialogs.rs` it cannot stay fully decoupled from the view that hosts it.
//!
//! # Focus
//!
//! The palette is exactly the overlay the focus-reclaim guard in
//! `WtmApp::render` warns about: its `TextInput` is mounted for the first
//! time on the same frame that opens it, so `WtmApp::overlay_open` must
//! already report `true` (via `self.palette.is_some()`) before that frame
//! renders, or the reclaim check would steal focus back to the root before
//! the palette's field ever gets it. See `app.rs`'s `on_open_palette` and
//! `overlay_open` for the two ends of that wiring.
//!
//! # Keyboard
//!
//! The search field's own keymap (`text_input::TextInput`) only binds
//! Enter (`InputEvent::Submit`) and Escape (`InputEvent::Cancel`) — both
//! wired through [`PaletteState::new`]'s subscription, the same shape
//! `dialogs::CreateState::new` uses for its fields. Up/Down are not part of
//! that keymap at all, so nothing would move the highlight without extra
//! wiring: `app.rs` attaches a raw `on_key_down` to the palette's card
//! (`WtmApp::on_palette_key_down`) that catches Up/Down directly, plus
//! ⌘+Enter — a keystroke distinct enough from plain Enter that it never
//! reaches `TextInput`'s "enter" binding at all (gpui keybindings match
//! modifiers exactly), so this is the only place that can see it.

use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, Context, Entity, ScrollHandle, SharedString, Subscription, Window,
};
use wtm::model::WorktreeInfo;

use crate::app::WtmApp;
use crate::assets::icons;
use crate::motion;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::{
    scrim, Theme, RADIUS_CONTROL, SCRIM_ALPHA_DARK, SPACE_12, SPACE_2, SPACE_4, SPACE_6, SPACE_8,
};
use crate::ui;

/// Width of the palette card. Deliberately wider than the ~400-440px
/// dialogs in `dialogs.rs`: a search result list reads two branch names or
/// a label-plus-shortcut pair per row, which needs more horizontal room
/// than a form field does.
const WIDTH: f32 = 560.0;
/// Distance from the top of the window to the palette's backdrop spacer —
/// a command palette anchored near the top (Spotlight/Sublime's "goto
/// anything" placement) reads as transient in a way a vertically centered
/// one does not, which matters here because the palette can jump-open
/// another repository's worktree out from under the user.
const TOP_OFFSET: f32 = 120.0;
const MAX_RESULTS_HEIGHT: f32 = 360.0;

// ---------------------------------------------------------------------
// Fuzzy scorer — pure, unit tested below.
// ---------------------------------------------------------------------

/// Base score for any matched character.
const BASE: i64 = 10;
/// Bonus for a character matched right at a word boundary (string start,
/// or right after `/`, `-`, `_`, `.`, ` `, or a lower-to-upper case
/// transition) — this is what lets `mwg` land on the initials of
/// `migrate`/`wtm`/`gpui` in `t3code/migrate-wtm-to-gpui-app` instead of
/// some earlier, less meaningful triple of letters.
const BOUNDARY_BONUS: i64 = 8;
/// Extra bonus when a match immediately follows the previous one — a
/// contiguous run reads as "found it" more than the same letters scattered
/// with gaps, even when both are otherwise equally boundary-aligned.
const CONSECUTIVE_BONUS: i64 = 6;

/// One scored match: `indices` are `char` offsets into `candidate`, in
/// increasing order, matched to `query`'s characters one-for-one — for
/// splitting the label into highlighted spans when rendering, and cheap
/// enough to always compute alongside the score since a palette's
/// candidate list is never more than a few hundred entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i64,
    pub indices: Vec<usize>,
}

/// Score `candidate` as a fuzzy subsequence match for `query`, or `None`
/// if `query`'s characters do not all appear in `candidate` in order.
/// Case-insensitive. An empty (or all-whitespace) `query` matches every
/// candidate with a flat score of `0` and no highlighted characters — the
/// palette's "browse everything" state before the user types anything.
///
/// This is a small dynamic-program: for each query character `i` and each
/// candidate position `j` it could land on, `dp[i][j]` holds the best total
/// score of a match ending exactly there, built from the best `dp[i-1][k]`
/// for any `k < j` (tracked as a running maximum while scanning `j` left to
/// right, so the whole thing stays O(query_len * candidate_len) rather than
/// the O(query_len * candidate_len^2) a naive "search every earlier k"
/// would be). Backpointers recover the actual matched indices once the best
/// final position is found.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    let query = query.trim();
    if query.is_empty() {
        return Some(FuzzyMatch {
            score: 0,
            indices: Vec::new(),
        });
    }

    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let c_lower: Vec<char> = candidate.chars().flat_map(char::to_lowercase).collect();
    let c_orig: Vec<char> = candidate.chars().collect();
    let qlen = q.len();
    let clen = c_lower.len();
    if qlen == 0 || qlen > clen {
        return None;
    }

    const NEG_INF: i64 = i64::MIN / 2;
    let mut dp: Vec<Vec<i64>> = vec![vec![NEG_INF; clen]; qlen];
    let mut back: Vec<Vec<usize>> = vec![vec![usize::MAX; clen]; qlen];

    for j in 0..clen {
        if c_lower[j] == q[0] {
            dp[0][j] = char_score(j, None, &c_orig);
        }
    }
    for i in 1..qlen {
        let mut running_best = NEG_INF;
        let mut running_best_k = usize::MAX;
        for j in 0..clen {
            if j > 0 && dp[i - 1][j - 1] > running_best {
                running_best = dp[i - 1][j - 1];
                running_best_k = j - 1;
            }
            if c_lower[j] != q[i] || running_best <= NEG_INF {
                continue;
            }
            dp[i][j] = running_best + char_score(j, Some(running_best_k), &c_orig);
            back[i][j] = running_best_k;
        }
    }

    let (best_score, best_j) = (0..clen)
        .filter(|&j| dp[qlen - 1][j] > NEG_INF)
        .map(|j| (dp[qlen - 1][j], j))
        .max_by_key(|(score, _)| *score)?;

    let mut indices = vec![0usize; qlen];
    let mut j = best_j;
    for i in (0..qlen).rev() {
        indices[i] = j;
        if i > 0 {
            j = back[i][j];
        }
    }
    Some(FuzzyMatch {
        score: best_score,
        indices,
    })
}

/// The score contribution of matching a single character at position `j`,
/// given the position it followed (`None` for the query's first
/// character).
fn char_score(j: usize, prev: Option<usize>, chars: &[char]) -> i64 {
    let is_boundary = j == 0
        || matches!(chars[j - 1], '/' | '-' | '_' | '.' | ' ')
        || (chars[j - 1].is_lowercase() && chars[j].is_uppercase());
    let mut score = BASE + if is_boundary { BOUNDARY_BONUS } else { 0 };
    score += match prev {
        Some(p) if j == p + 1 => CONSECUTIVE_BONUS,
        Some(p) => -((j - p - 1) as i64),
        None => -(j as i64),
    };
    score
}

// ---------------------------------------------------------------------
// Commands table
// ---------------------------------------------------------------------

/// One of the app's actions, as offered in the palette's "Commands"
/// section. `app.rs`'s `WtmApp::run_palette_command` is the other half of
/// this table: every variant here has exactly one arm there, dispatching
/// through the same `WtmApp::on_*` method the real keystroke or button
/// already calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    NewWorktree,
    RemoveWorktree,
    Prune,
    Reload,
    OpenEditor,
    OpenTerminal,
    RevealFinder,
    CopyPath,
    ToggleSidebar,
    ToggleDetailPanel,
    Settings,
    FetchRemote,
    AddRepository,
    ShowDetailsTab,
    ShowFilesTab,
    ShowChangesTab,
    RunCommand,
    OpenRemote,
}

#[derive(Clone, Copy)]
pub struct CommandSpec {
    pub id: CommandId,
    pub label: &'static str,
    /// Mirrors the glyph shown for this action's real keystroke in
    /// `main.rs`'s `key_bindings!` table — kept as a separate literal
    /// rather than looked up from `REGISTERED_BINDINGS` by label text,
    /// since a string-matched lookup would be its own, subtler source of
    /// drift if a label ever changed on one side and not the other. Empty
    /// (`""`) for a command with no keyboard binding at all (e.g.
    /// `OpenRemote`) — `render_entry` skips the shortcut chip entirely in
    /// that case rather than showing an empty one.
    pub shortcut: &'static str,
    pub icon: &'static str,
}

/// Every command the palette offers, in the order shown for an empty
/// query. "Open in Terminal" has no dedicated icon in `assets.rs` — same
/// tradeoff `app.rs`'s title bar already makes for the detail-panel
/// toggle — so it reuses [`icons::OPEN_EXTERNAL`], the closest available
/// fit for "launches something outside the window."
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: CommandId::NewWorktree,
        label: "New Worktree",
        shortcut: "⌘N",
        icon: icons::PLUS,
    },
    CommandSpec {
        id: CommandId::RemoveWorktree,
        label: "Remove Worktree",
        shortcut: "⌘⌫",
        icon: icons::TRASH,
    },
    CommandSpec {
        id: CommandId::Prune,
        label: "Prune…",
        shortcut: "⌘⇧P",
        icon: icons::TRASH,
    },
    CommandSpec {
        id: CommandId::Reload,
        label: "Reload",
        shortcut: "⌘R",
        icon: icons::REFRESH,
    },
    CommandSpec {
        id: CommandId::OpenEditor,
        label: "Open in Editor",
        shortcut: "⏎",
        icon: icons::OPEN_EXTERNAL,
    },
    CommandSpec {
        id: CommandId::OpenTerminal,
        label: "Open in Terminal",
        shortcut: "⌘⇧T",
        icon: icons::OPEN_EXTERNAL,
    },
    CommandSpec {
        id: CommandId::RevealFinder,
        label: "Reveal in Finder",
        shortcut: "⌘⇧R",
        icon: icons::FOLDER,
    },
    CommandSpec {
        id: CommandId::CopyPath,
        label: "Copy Path",
        shortcut: "⌘C",
        icon: icons::COPY,
    },
    CommandSpec {
        id: CommandId::ToggleSidebar,
        label: "Toggle Sidebar",
        shortcut: "⌘B",
        icon: icons::PANEL_LEFT,
    },
    CommandSpec {
        id: CommandId::ToggleDetailPanel,
        label: "Toggle Detail Panel",
        shortcut: "⌘I",
        icon: icons::PANEL_LEFT,
    },
    CommandSpec {
        id: CommandId::Settings,
        label: "Settings",
        shortcut: "⌘,",
        icon: icons::SETTINGS,
    },
    CommandSpec {
        id: CommandId::FetchRemote,
        label: "Fetch",
        shortcut: "⌘⇧F",
        icon: icons::REFRESH,
    },
    CommandSpec {
        id: CommandId::AddRepository,
        label: "Add Repository…",
        shortcut: "⌘⇧O",
        icon: icons::PLUS,
    },
    CommandSpec {
        id: CommandId::ShowDetailsTab,
        label: "Detail Panel: Details Tab",
        shortcut: "⌘1",
        icon: icons::PANEL_LEFT,
    },
    CommandSpec {
        id: CommandId::ShowFilesTab,
        label: "Detail Panel: Files Tab",
        shortcut: "⌘2",
        icon: icons::FOLDER,
    },
    CommandSpec {
        id: CommandId::ShowChangesTab,
        label: "Detail Panel: Changes Tab",
        shortcut: "⌘3",
        icon: icons::GIT_BRANCH,
    },
    CommandSpec {
        id: CommandId::RunCommand,
        label: "Run Command…",
        shortcut: "⌘E",
        icon: icons::CHECK,
    },
    CommandSpec {
        id: CommandId::OpenRemote,
        label: "Open on Remote…",
        // No keyboard binding — see `app::commands::open_remote_menu_item`'s
        // doc comment on why this action's availability depends on the
        // selected worktree's branch/remote and so isn't a good fit for a
        // fixed, always-on global shortcut.
        shortcut: "",
        icon: icons::OPEN_EXTERNAL,
    },
];

// ---------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------

/// One ranked, ready-to-render result — either a worktree to jump to, or a
/// command to run.
pub enum PaletteEntry {
    Worktree {
        row_ix: usize,
        label: String,
        indices: Vec<usize>,
    },
    Command {
        spec: CommandSpec,
        indices: Vec<usize>,
    },
}

/// Rank `rows` and [`COMMANDS`] against `query`, worktrees first — "Empty
/// query shows worktrees first (most useful default), then commands" is
/// just this ordering falling out of the two sections being concatenated
/// in that order, both scored (and stably sorted, so ties keep `rows`'/
/// `COMMANDS`' own order) the same way.
pub fn compute_results(query: &str, rows: &[WorktreeInfo]) -> Vec<PaletteEntry> {
    let mut worktrees: Vec<(i64, PaletteEntry)> = rows
        .iter()
        .enumerate()
        .filter_map(|(row_ix, row)| {
            let label = row.display_name().to_string();
            let m = fuzzy_match(query, &label)?;
            Some((
                m.score,
                PaletteEntry::Worktree {
                    row_ix,
                    label,
                    indices: m.indices,
                },
            ))
        })
        .collect();
    worktrees.sort_by_key(|(score, _)| std::cmp::Reverse(*score));

    let mut commands: Vec<(i64, PaletteEntry)> = COMMANDS
        .iter()
        .filter_map(|spec| {
            let m = fuzzy_match(query, spec.label)?;
            Some((
                m.score,
                PaletteEntry::Command {
                    spec: *spec,
                    indices: m.indices,
                },
            ))
        })
        .collect();
    commands.sort_by_key(|(score, _)| std::cmp::Reverse(*score));

    worktrees
        .into_iter()
        .chain(commands)
        .map(|(_, e)| e)
        .collect()
}

/// Position of the flat result index `highlighted` (as `compute_results`
/// numbers it: worktrees then commands, see that function's own doc) within
/// the `results_list` div `render` actually paints. `render` opens each
/// group with no eyebrow label; the only extra DOM child is a single
/// `ui::divider` between the two groups, and only when both are non-empty.
/// So the DOM child index matches the flat result index exactly until
/// `highlighted` reaches the commands section, where it is offset by one
/// for that divider (but only if there were worktrees ahead of it to
/// divide from). Pure, and given plain counts rather than the
/// `PaletteEntry` list itself, so `palette_move_highlight` can call it
/// without `render`'s own borrow of `self.palette`. Returns `None` for an
/// out-of-range `highlighted` (an empty results list, or a stale highlight
/// left over from a shorter query).
fn results_scroll_child_index(
    worktree_count: usize,
    command_count: usize,
    highlighted: usize,
) -> Option<usize> {
    if highlighted >= worktree_count + command_count {
        return None;
    }
    if highlighted < worktree_count {
        Some(highlighted)
    } else {
        let divider_offset = if worktree_count > 0 { 1 } else { 0 };
        Some(worktree_count + divider_offset + (highlighted - worktree_count))
    }
}

// ---------------------------------------------------------------------
// State
// ---------------------------------------------------------------------

pub struct PaletteState {
    pub input: Entity<TextInput>,
    // Held only to keep the subscription alive — see `dialogs::CreateState`
    // for the same convention.
    _input_sub: Subscription,
    /// Index into the *flat* result list `compute_results` produces
    /// (worktrees then commands) — recomputed at render time, not stored,
    /// so this is the only piece of navigation state that persists between
    /// keystrokes.
    pub highlighted: usize,
    /// The results column's own scroll position — `ui::scrollbar` needs a
    /// handle that survives across renders.
    scroll: ScrollHandle,
}

impl PaletteState {
    pub fn new(window: &mut Window, cx: &mut Context<WtmApp>) -> Self {
        // `.borderless()`: the palette's search field sits inside the
        // `ui::popover` card's own well — a borderless inset well — rather
        // than drawing a second box of its own — see
        // `TextInput::borderless`'s doc.
        let input =
            cx.new(|cx| TextInput::new("Search worktrees and commands…", window, cx).borderless());
        let sub = cx.subscribe_in(&input, window, {
            // Only ever calls back through `WtmApp`'s own `pub(crate)`
            // methods, never reaches into its fields directly — the same
            // discipline `dialogs::CreateState::new`'s subscription
            // follows, which is what lets `WtmApp`'s fields (including
            // `palette` itself) stay private.
            move |app: &mut WtmApp, _input, event, window, cx| match event {
                InputEvent::Submit => {
                    // Plain Enter: select-and-close only, never open — see
                    // the module doc on why `on_palette_key_down` is the
                    // only path that can open (⌘+Enter), and `app.rs`'s
                    // `palette_activate` for the full reasoning.
                    app.palette_activate_highlighted(false, window, cx);
                }
                InputEvent::Cancel => app.close_palette(window, cx),
                InputEvent::Changed => {
                    // A new query invalidates whatever the old highlight
                    // pointed at (the result under it may not even exist
                    // anymore) — resetting to the top result matches every
                    // other fuzzy picker's behavior and needs no knowledge
                    // of the new result count to stay in bounds.
                    app.palette_reset_highlight(cx);
                }
            }
        });
        Self {
            input,
            _input_sub: sub,
            highlighted: 0,
            scroll: ScrollHandle::new(),
        }
    }

    /// Scroll `self.scroll` so the currently highlighted result is inside
    /// the results column's viewport — Bug 3's "arrow keys scroll the
    /// selection into view" rule, extended to the palette's own list.
    /// `worktree_count`/`command_count` describe the *current* query's
    /// results (`palette_move_highlight` computed them a moment ago to
    /// clamp `highlighted` itself); `results_scroll_child_index` is the
    /// pure translation from flat result index to DOM child index.
    /// `ScrollHandle::scroll_to_item`'s default strategy (`FirstVisible`)
    /// already no-ops when the row is already on screen, so this is safe
    /// to call unconditionally.
    pub(crate) fn scroll_highlighted_into_view(&self, worktree_count: usize, command_count: usize) {
        if let Some(child_ix) =
            results_scroll_child_index(worktree_count, command_count, self.highlighted)
        {
            self.scroll.scroll_to_item(child_ix);
        }
    }
}

// ---------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------

/// Render the palette overlay. `rows` is `WtmApp::rows` (handed in rather
/// than read off an entity this module doesn't own) so the worktree
/// section always reflects whatever the list is currently showing,
/// including a background reload that lands while the palette is open.
pub fn render(
    state: &PaletteState,
    rows: &[WorktreeInfo],
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> AnyElement {
    let query = state.input.read(cx).value().to_string();
    let results = compute_results(&query, rows);
    let highlighted = state.highlighted.min(results.len().saturating_sub(1));

    let (worktree_entries, command_entries): (Vec<(usize, &PaletteEntry)>, Vec<_>) = results
        .iter()
        .enumerate()
        .partition(|(_, e)| matches!(e, PaletteEntry::Worktree { .. }));

    let results_list = div()
        .id("palette-results")
        .flex()
        .flex_col()
        .gap(px(SPACE_2))
        .max_h(px(MAX_RESULTS_HEIGHT))
        .overflow_y_scroll()
        .track_scroll(&state.scroll)
        .px(px(SPACE_6))
        .py(px(SPACE_6))
        .when(!worktree_entries.is_empty(), |this| {
            this.children(
                worktree_entries
                    .iter()
                    .map(|(ix, e)| render_entry(*ix, e, *ix == highlighted, theme, cx)),
            )
        })
        // No "Worktrees" / "Commands" eyebrow labels: a hairline divider
        // between the groups (only when both are present) carries the
        // grouping instead, alongside the per-row icon that already differs
        // (branch glyph vs. command glyph, see `render_entry`).
        .when(
            !worktree_entries.is_empty() && !command_entries.is_empty(),
            |this| {
                this.child(
                    div()
                        .px(px(SPACE_6))
                        .py(px(SPACE_4))
                        .child(ui::divider(theme)),
                )
            },
        )
        .when(!command_entries.is_empty(), |this| {
            this.children(
                command_entries
                    .iter()
                    .map(|(ix, e)| render_entry(*ix, e, *ix == highlighted, theme, cx)),
            )
        })
        .when(results.is_empty(), |this| {
            // A one-line empty state, not the full icon+headline
            // `ui::empty_state` — that component is sized for a panel
            // filling its own space, which would dwarf a "no matches" hint
            // inside an already-open search overlay. Same primitives
            // (`ui::icon`, muted text), composed at a scale that fits here.
            this.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(SPACE_6))
                    .px(px(SPACE_8))
                    .py(px(SPACE_8))
                    .text_size(px(ui::TEXT_SM))
                    .text_color(theme.text_muted)
                    .child(ui::icon(icons::SEARCH, 13.0, theme.text_faint))
                    .child("No matches"),
            )
        });

    // `.relative()` wrapper, sibling of `results_list` itself — same
    // reasoning as `app::chrome`'s scroll regions (`ui::scrollbar`'s own
    // doc): the overlay must never be a descendant of the div it scrolls
    // with, or it scrolls away with the very results it's annotating.
    let results_col = div().relative().child(results_list).child(ui::scrollbar(
        "palette-results-scrollbar",
        &state.scroll,
        ui::ScrollAxis::Vertical,
    ));

    // Search field: a borderless inset well with a leading search icon.
    // `TextInput` itself paints no background/border in `.borderless()`
    // mode (see `PaletteState::new`), so this wrapper is the well:
    // `surface_inset` at `RADIUS_CONTROL`, concentric with the card's own
    // `RADIUS_PANEL` (10) at `SPACE_4` (4) padding: `10 - 4 == 6 ==
    // RADIUS_CONTROL`.
    let search = div().p(px(SPACE_4)).child(
        div()
            .id("palette-search")
            .flex()
            .items_center()
            .gap(px(SPACE_8))
            .h(px(ui::ROW_HEIGHT))
            .px(px(SPACE_12))
            .rounded(px(RADIUS_CONTROL))
            .bg(theme.surface_inset)
            .child(ui::icon(icons::SEARCH, 14.0, theme.text_faint))
            .child(div().flex_1().min_w_0().child(state.input.clone())),
    );

    // `ui::popover`: `RADIUS_PANEL` + `shadow_popover`, the same overlay
    // surface the context menu uses — not `ui::modal_card`
    // (`RADIUS_DIALOG`/`shadow_dialog`), which is the dialog ladder's step,
    // not the popover ladder's.
    let card = ui::popover(theme)
        .id("palette-card")
        .w(px(WIDTH))
        .on_click(|_, _, cx| cx.stop_propagation())
        .on_key_down(cx.listener(WtmApp::on_palette_key_down))
        .child(search)
        .child(ui::divider(theme))
        .child(results_col);

    // The results list beneath never animates (touched on every
    // keystroke); the palette itself is touched rarely and enters with
    // `MENU_IN` so the motion tells the eye where it came from.
    let card = motion::menu_in("palette-card-motion", card, cx);

    div()
        .id("palette-backdrop")
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .items_center()
        .bg(scrim(SCRIM_ALPHA_DARK))
        // Covers the whole window; without this a scroll wheel anywhere
        // over the backdrop (or over `card`, which itself occludes via
        // `ui::popover`) would fall through to the worktree list behind
        // it — see `ui::modal_backdrop`'s doc for the same reasoning.
        .occlude()
        .on_click(cx.listener(|this, _, window, cx| this.close_palette(window, cx)))
        .child(div().h(px(TOP_OFFSET)).flex_none())
        .child(card)
        .into_any_element()
}

fn render_entry(
    ix: usize,
    entry: &PaletteEntry,
    highlighted: bool,
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> AnyElement {
    let (icon_path, label, indices, shortcut) = match entry {
        PaletteEntry::Worktree { label, indices, .. } => {
            (icons::GIT_BRANCH, label.as_str(), indices.as_slice(), None)
        }
        PaletteEntry::Command { spec, indices } => (
            spec.icon,
            spec.label,
            indices.as_slice(),
            // Empty means "no keyboard binding" — see `CommandSpec::shortcut`'s
            // doc comment — so no chip is shown at all rather than an empty one.
            (!spec.shortcut.is_empty()).then(|| SharedString::from(spec.shortcut)),
        ),
    };

    ui::row(("palette-entry", ix), highlighted, theme)
        .flex()
        .items_center()
        .gap(px(SPACE_8))
        .child(ui::icon(icon_path, 13.0, theme.text_faint))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .flex()
                .items_center()
                .children(highlighted_spans(label, indices, theme)),
        )
        // A small "press Enter" hint on the highlighted row only — reuses
        // `icons::ENTER`, otherwise unused anywhere in the app (`assets.rs`
        // embeds it but nothing consumed it before this).
        .when(highlighted, |this| {
            this.child(ui::icon(icons::ENTER, 11.0, theme.text_ghost))
        })
        .when_some(shortcut, |this, hint| this.child(ui::kbd(&hint, theme)))
        .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
            if *hovered {
                this.palette_set_highlight(ix, cx);
            }
        }))
        .on_click(
            cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                // See the module doc / `palette_activate`: plain click behaves
                // like plain Enter (select-and-close only); holding ⌘ also
                // opens a worktree result in the editor. Ignored for command
                // results — `palette_activate` never reads it for those.
                this.palette_activate(ix, event.modifiers().platform, window, cx);
            }),
        )
        .into_any_element()
}

/// Split `label` into contiguous runs at the boundaries between matched
/// and unmatched characters, rendered as a row of differently-colored text
/// spans — matched runs in `theme.accent`, the rest in `theme.text`. Works
/// in `char` space throughout (matching `fuzzy_match`'s `indices`), which
/// sidesteps ever slicing a `String` at a non-UTF8-boundary byte offset.
fn highlighted_spans(label: &str, indices: &[usize], theme: &Theme) -> Vec<AnyElement> {
    if indices.is_empty() {
        return vec![span(label, false, theme)];
    }
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_matched = false;
    for (i, ch) in label.chars().enumerate() {
        // `indices` is already sorted (`FuzzyMatch`'s own doc), so a binary
        // search finds a match in O(log n) with no set to build.
        let is_matched = indices.binary_search(&i).is_ok();
        if !run.is_empty() && is_matched != run_matched {
            spans.push(span(&run, run_matched, theme));
            run.clear();
        }
        run.push(ch);
        run_matched = is_matched;
    }
    if !run.is_empty() {
        spans.push(span(&run, run_matched, theme));
    }
    spans
}

fn span(text: &str, matched: bool, theme: &Theme) -> AnyElement {
    div()
        .flex_none()
        .text_size(px(ui::TEXT_BASE))
        .text_color(if matched { theme.accent } else { theme.text })
        .child(text.to_string())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indices_of(query: &str, candidate: &str) -> Vec<usize> {
        fuzzy_match(query, candidate)
            .unwrap_or_else(|| panic!("expected {query:?} to match {candidate:?}"))
            .indices
    }

    #[test]
    fn non_subsequence_does_not_match() {
        assert_eq!(fuzzy_match("xyz", "abcdef"), None);
    }

    #[test]
    fn query_longer_than_candidate_does_not_match() {
        assert_eq!(fuzzy_match("abcdef", "abc"), None);
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(fuzzy_match("ABC", "abcdef").is_some());
        assert!(fuzzy_match("abc", "ABCDEF").is_some());
    }

    #[test]
    fn empty_query_matches_everything_with_no_highlights() {
        let m = fuzzy_match("", "anything").unwrap();
        assert_eq!(m.score, 0);
        assert!(m.indices.is_empty());

        let m = fuzzy_match("   ", "anything").unwrap();
        assert_eq!(m.score, 0);
    }

    #[test]
    fn matched_indices_are_strictly_increasing_and_in_bounds() {
        for (q, c) in [
            ("mwg", "t3code/migrate-wtm-to-gpui-app"),
            ("nw", "New Worktree"),
            ("ab", "aabb"),
        ] {
            let indices = indices_of(q, c);
            assert_eq!(indices.len(), q.chars().count());
            for w in indices.windows(2) {
                assert!(w[0] < w[1], "indices must strictly increase: {indices:?}");
            }
            assert!(*indices.last().unwrap() < c.chars().count());
        }
    }

    /// The flagship example from the task spec: `mwg` should land on the
    /// initials of the three word-boundary segments `migrate`, `wtm`, and
    /// `gpui`, not on some earlier, less meaningful triple of letters (the
    /// candidate also contains an early, non-boundary `m`... `g` pair
    /// inside "migrate" itself).
    #[test]
    fn subsequence_query_prefers_word_boundaries() {
        let candidate = "t3code/migrate-wtm-to-gpui-app";
        // m(igrate) w(tm) g(pui): indices of 'm', 'w', 'g' right after '/',
        // '-', '-' respectively.
        assert_eq!(indices_of("mwg", candidate), vec![7, 15, 22]);
    }

    #[test]
    fn word_initial_match_outranks_a_closer_mid_word_letter() {
        // "New Worktree": query "nw" could match n(ew)+w(ithin "New") or
        // n(ew)+W(orktree). The second is a word boundary and, despite
        // being farther from `n`, must win.
        assert_eq!(indices_of("nw", "New Worktree"), vec![0, 4]);
    }

    #[test]
    fn earlier_match_outranks_a_later_one_when_neither_is_a_boundary() {
        let early = fuzzy_match("a", "zzazzz").unwrap();
        let late = fuzzy_match("a", "zzzzza").unwrap();
        assert!(
            early.score > late.score,
            "an earlier non-boundary match should still outscore a later one"
        );
    }

    #[test]
    fn boundary_match_can_outrank_a_slightly_earlier_non_boundary_one() {
        let non_boundary = fuzzy_match("c", "acxxxxxx").unwrap(); // c at index 1
        let boundary = fuzzy_match("c", "ax-cxxxx").unwrap(); // c at index 3, right after '-'
        assert!(
            boundary.score > non_boundary.score,
            "a word-boundary match should be able to outrank an earlier, buried one"
        );
    }

    #[test]
    fn consecutive_run_outranks_the_same_letters_scattered() {
        let consecutive = fuzzy_match("ab", "ab--------").unwrap();
        let scattered = fuzzy_match("ab", "a--------b").unwrap();
        assert!(consecutive.score > scattered.score);
    }

    #[test]
    fn compute_results_orders_worktrees_before_commands_on_empty_query() {
        let rows = sample_rows();
        let results = compute_results("", &rows);
        let first_command = results
            .iter()
            .position(|e| matches!(e, PaletteEntry::Command { .. }));
        let last_worktree = results
            .iter()
            .rposition(|e| matches!(e, PaletteEntry::Worktree { .. }));
        assert_eq!(results.len(), rows.len() + COMMANDS.len());
        assert!(last_worktree.unwrap() < first_command.unwrap());
    }

    #[test]
    fn compute_results_filters_out_non_matching_worktrees() {
        let rows = sample_rows();
        let results = compute_results("alpha", &rows);
        let worktree_labels: Vec<&str> = results
            .iter()
            .filter_map(|e| match e {
                PaletteEntry::Worktree { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(worktree_labels, vec!["feature/alpha"]);
    }

    #[test]
    fn compute_results_can_match_commands_by_label() {
        let results = compute_results("prune", &[]);
        assert!(results.iter().any(
            |e| matches!(e, PaletteEntry::Command { spec, .. } if spec.id == CommandId::Prune)
        ));
    }

    fn sample_rows() -> Vec<WorktreeInfo> {
        vec![
            row("main", true),
            row("feature/alpha", false),
            row("feature/beta", false),
        ]
    }

    fn row(branch: &str, is_main: bool) -> WorktreeInfo {
        WorktreeInfo {
            name: branch.to_string(),
            path: std::path::PathBuf::from(format!("/tmp/{branch}")),
            branch: Some(branch.to_string()),
            head: None,
            is_main,
            is_missing: false,
            is_locked: false,
            is_prunable: false,
            status: None,
        }
    }

    // -------------------------------------------------------------
    // `results_scroll_child_index` — Bug 3: palette highlight scrolling
    // -------------------------------------------------------------

    #[test]
    fn results_scroll_child_index_matches_the_flat_index_within_worktrees() {
        // 2 worktrees, 1 command: DOM is [wt0, wt1, divider, cmd0].
        assert_eq!(results_scroll_child_index(2, 1, 0), Some(0));
        assert_eq!(results_scroll_child_index(2, 1, 1), Some(1));
    }

    #[test]
    fn results_scroll_child_index_skips_the_divider_for_a_command() {
        assert_eq!(results_scroll_child_index(2, 1, 2), Some(3));
    }

    #[test]
    fn results_scroll_child_index_has_no_divider_when_worktrees_is_empty() {
        // DOM is [cmd0, cmd1] — no worktrees section, so no divider either.
        assert_eq!(results_scroll_child_index(0, 2, 0), Some(0));
        assert_eq!(results_scroll_child_index(0, 2, 1), Some(1));
    }

    #[test]
    fn results_scroll_child_index_is_none_when_out_of_range() {
        assert_eq!(results_scroll_child_index(2, 1, 3), None);
        assert_eq!(results_scroll_child_index(0, 0, 0), None);
    }
}

//! Width-adaptive layout decisions for the chrome shell.
//!
//! The window has a real, enforced minimum (820×520 — `main.rs`'s
//! `window_min_size`), but until this module existed nothing in the app
//! actually adapted to it: the sidebar (`theme::SIDEBAR_WIDTH`, 248) and the
//! detail panel (`detail_panel::WIDTH`/`WIDE_WIDTH`, 320/640) were both
//! fixed, so at the minimum window with both panes open the worktree
//! list — the thing the app is *for* — was left with as little as
//! `820 - 248 - 320 = 252px`, and with the Files/Changes tabs' wide panel,
//! a literally negative column.
//!
//! Every decision below is a pure function of a window width (plus, where
//! it matters, a small bit of explicit state) so it can be unit-tested
//! without a live `Window` — `chrome.rs`/`commands.rs`/`mod.rs` call these
//! and paint the result; they never re-derive the arithmetic themselves.

use crate::detail_panel;
use crate::theme;

/// The content-column width the codebase already treats as "usable" for
/// the worktree list, reused rather than inventing a second, competing
/// minimum: `detail_panel::WIDE_WIDTH`'s own doc calls the column left over
/// at the app's default window size (`crate::DEFAULT_WINDOW_SIZE.0`, 1180),
/// with the *wide* Files/Changes panel showing, "a usable width" — that is
/// exactly `1180 - SIDEBAR_WIDTH - WIDE_WIDTH` = 292px. Defining it this
/// way means both breakpoints below are anchored to a number this codebase
/// has already implicitly relied on, not a fresh guess.
pub const MIN_CONTENT_COLUMN: f32 =
    crate::DEFAULT_WINDOW_SIZE.0 - theme::SIDEBAR_WIDTH - detail_panel::WIDE_WIDTH;

/// Window width below which the detail panel — at its normal Details-tab
/// width — can no longer sit next to the sidebar without squeezing the
/// worktree list under [`MIN_CONTENT_COLUMN`]. Below this, the panel
/// auto-collapses (see [`detail_panel_should_show`]).
///
/// Arithmetic: `SIDEBAR_WIDTH` (248) + `detail_panel::WIDTH` (320) +
/// `MIN_CONTENT_COLUMN` (292) = **860px**. Recomputed from those three
/// constants, so it moves if any of them do.
pub const DETAIL_PANEL_BREAKPOINT: f32 =
    theme::SIDEBAR_WIDTH + detail_panel::WIDTH + MIN_CONTENT_COLUMN;

/// Window width below which even the *wide* Files/Changes panel
/// (`detail_panel::WIDE_WIDTH`) can't be shown without the same squeeze.
///
/// Arithmetic: `SIDEBAR_WIDTH` (248) + `detail_panel::WIDE_WIDTH` (640) +
/// `MIN_CONTENT_COLUMN` (292) = **1180px** — which is exactly
/// `crate::DEFAULT_WINDOW_SIZE.0` by construction, since
/// [`MIN_CONTENT_COLUMN`] was itself defined as the slack left over at that
/// width. In other words: the Files/Changes tabs are only guaranteed a
/// usable list at the app's own default window size or wider; narrower
/// than that, they are unreachable (see [`wide_tabs_fit`]) rather than
/// rendered with a crushed or negative-width list.
pub const WIDE_TABS_BREAKPOINT: f32 =
    theme::SIDEBAR_WIDTH + detail_panel::WIDE_WIDTH + MIN_CONTENT_COLUMN;

/// The worktree list's own content column for one frame: the window's
/// viewport width, minus the sidebar when it's shown, minus the detail
/// panel's own current width when *it's* shown. Shared by
/// `chrome::WtmApp::worktree_row_card_width` (which further subtracts the
/// list's own fixed chrome) and `chrome::render_footer` (which uses it
/// directly — the footer spans the same column, see `app/mod.rs`'s root
/// layout) so both ask the same question about the same number instead of
/// keeping two copies of this subtraction that could drift apart.
pub fn content_column_width(
    window_width: f32,
    sidebar_visible: bool,
    detail_panel_width: Option<f32>,
) -> f32 {
    let mut width = window_width;
    if sidebar_visible {
        width -= theme::SIDEBAR_WIDTH;
    }
    if let Some(panel) = detail_panel_width {
        width -= panel;
    }
    width
}

/// Whether the detail panel should actually be visible this frame, folding
/// together the user's own toggle preference and the width-driven
/// auto-collapse without ever letting the two fight each other.
///
/// Modeled as two independent bits, both owned by `WtmApp`:
/// - `user_wants_open` (`WtmApp::detail_panel_visible`, persisted in
///   `Prefs`): the user's explicit preference, changed only by the
///   sidebar/detail toggle or its keybinding. Auto-collapse never touches
///   this — a pane the width hid is still a pane the user asked for.
/// - `narrow_override` (`WtmApp::detail_panel_narrow_override`, session-only,
///   never persisted): true exactly when the user has explicitly reopened
///   the panel *while already narrow* (see `commands::on_toggle_detail_panel`)
///   — recorded so this same auto-collapse doesn't immediately slam the
///   panel shut again on the very next render, fighting the click that just
///   happened.
///
/// The two combine as: show it if the user wants it open, AND (there's
/// room, OR the user just overrode the lack of room). Widening the window
/// past [`DETAIL_PANEL_BREAKPOINT`] always shows a wanted-open panel again
/// on its own — the "or there's room" branch — which is what makes a
/// width-collapsed (as opposed to user-closed) pane restore when the
/// window widens back out.
pub fn detail_panel_should_show(
    window_width: f32,
    user_wants_open: bool,
    narrow_override: bool,
) -> bool {
    user_wants_open && (window_width >= DETAIL_PANEL_BREAKPOINT || narrow_override)
}

/// The other half of [`detail_panel_should_show`]'s `narrow_override` bit:
/// called once per render (`WtmApp::render`, before building the tree) to
/// decide whether the override should still be held.
///
/// The override is kept only while the window is *still* narrow — the
/// instant the window is wide enough that [`detail_panel_should_show`]
/// would show the panel anyway (the "or there's room" branch), the override
/// stops doing anything and is dropped. That is deliberate, not just tidy
/// bookkeeping: without it, one explicit reopen at a narrow width would
/// silently exempt the panel from auto-collapsing forever, in every future
/// narrow session, which is a much bigger promise than "I want it open
/// right now despite the squeeze" — the thing the click actually meant.
/// Dropping it here means the *next* time the window narrows, auto-collapse
/// applies fresh, exactly as if the override had never happened.
pub fn narrow_override_after_resize(window_width: f32, override_was_set: bool) -> bool {
    override_was_set && window_width < DETAIL_PANEL_BREAKPOINT
}

/// Whether the Files/Changes tabs' wide panel (`detail_panel::WIDE_WIDTH`)
/// fits at this window width — see [`WIDE_TABS_BREAKPOINT`]'s doc for why
/// there is no user override for this one the way there is for
/// [`detail_panel_should_show`]'s `narrow_override`: below this width the
/// list column the wide panel would leave behind is not just tight, it can
/// go negative (at the 820px minimum: `820 - 248 - 640 = -68`), which is a
/// structural impossibility, not a tradeoff a click can opt back into.
pub fn wide_tabs_fit(window_width: f32) -> bool {
    window_width >= WIDE_TABS_BREAKPOINT
}

/// The footer's hint row (`chrome::render_footer_hints`), by priority.
/// `↑↓ select` names the list's single most fundamental interaction and is
/// kept down to the narrowest width the footer ever actually renders at;
/// `⌘R reload` is dropped first because it duplicates the titlebar's own
/// reload button one glance away, so losing it costs the least.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterHints {
    /// `↑↓ select`, `⏎ open in editor`, `⌘R reload`.
    All,
    /// `↑↓ select`, `⏎ open in editor` — reload dropped.
    Core,
    /// `↑↓ select` only.
    Minimal,
    /// No room for even one hint chip at this width.
    None,
}

impl FooterHints {
    /// Picks a tier from the footer's own content-column width (see
    /// [`content_column_width`]) — every hint here is a short, fixed,
    /// compile-time-known string rather than user content (a path, a
    /// commit subject), so there is no per-glyph truncation story to lean
    /// on the way `detail_panel::LABEL_WIDTH` documents for those; the only
    /// thing that scales at a narrow width is how many *whole* hints are
    /// shown, never a clipped one.
    ///
    /// The thresholds are a conservative, documented approximation in the
    /// same spirit as `worktree_list::CHAR_WIDTH_APPROX` (gpui has no API
    /// to measure real shaped text outside of an actual layout pass): each
    /// tier's own three/two/one hint chips (kbd cap + label, `SPACE_6` gaps
    /// between every child) comfortably fit within it with room to spare
    /// for the footer's `SPACE_16` side padding and the trailing repo/branch
    /// chips on the same row, and each tier remains comfortably below the
    /// next narrower breakpoint above it — so a resize never lands exactly
    /// on a boundary and flickers between tiers.
    pub fn for_content_column(content_column: f32) -> Self {
        if content_column >= 480.0 {
            FooterHints::All
        } else if content_column >= 360.0 {
            FooterHints::Core
        } else if content_column >= 260.0 {
            FooterHints::Minimal
        } else {
            FooterHints::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Breakpoint arithmetic
    // -----------------------------------------------------------------

    #[test]
    fn min_content_column_matches_the_wide_panel_slack_at_the_default_width() {
        assert_eq!(MIN_CONTENT_COLUMN, 292.0);
    }

    #[test]
    fn detail_panel_breakpoint_is_derived_from_the_three_named_constants() {
        assert_eq!(
            DETAIL_PANEL_BREAKPOINT,
            theme::SIDEBAR_WIDTH + detail_panel::WIDTH + MIN_CONTENT_COLUMN
        );
        assert_eq!(DETAIL_PANEL_BREAKPOINT, 860.0);
    }

    #[test]
    fn wide_tabs_breakpoint_equals_the_default_window_width() {
        assert_eq!(
            WIDE_TABS_BREAKPOINT,
            theme::SIDEBAR_WIDTH + detail_panel::WIDE_WIDTH + MIN_CONTENT_COLUMN
        );
        assert_eq!(WIDE_TABS_BREAKPOINT, crate::DEFAULT_WINDOW_SIZE.0);
    }

    #[test]
    fn true_minimum_window_is_below_the_detail_panel_breakpoint() {
        // The whole point of this module: 820 (the enforced minimum) must
        // never satisfy `detail_panel_should_show`'s "there's room" branch.
        // `DETAIL_PANEL_BREAKPOINT` is itself a `const`, so clippy treats a
        // direct `assert!` against it as an always-true tautology and
        // refuses to compile it (`assertions_on_constants`); routing the
        // 820.0 literal through a non-const local sidesteps that lint
        // without weakening what's actually being checked.
        let true_minimum_window: f32 = 820.0;
        assert!(true_minimum_window < DETAIL_PANEL_BREAKPOINT);
    }

    // -----------------------------------------------------------------
    // content_column_width
    // -----------------------------------------------------------------

    #[test]
    fn content_column_subtracts_visible_panes_only() {
        assert_eq!(content_column_width(820.0, false, None), 820.0);
        assert_eq!(content_column_width(820.0, true, None), 820.0 - 248.0);
        assert_eq!(
            content_column_width(820.0, true, Some(320.0)),
            820.0 - 248.0 - 320.0
        );
        assert_eq!(
            content_column_width(820.0, false, Some(320.0)),
            820.0 - 320.0
        );
    }

    #[test]
    fn content_column_can_go_negative_for_the_wide_panel_at_the_minimum_width() {
        // Documents the exact "-68" the module doc cites for why
        // `wide_tabs_fit` has no user override.
        assert_eq!(content_column_width(820.0, true, Some(640.0)), -68.0);
    }

    // -----------------------------------------------------------------
    // detail_panel_should_show / narrow_override_after_resize
    // -----------------------------------------------------------------

    #[test]
    fn user_closed_panel_never_shows_regardless_of_width_or_override() {
        assert!(!detail_panel_should_show(1920.0, false, false));
        assert!(!detail_panel_should_show(1920.0, false, true));
        assert!(!detail_panel_should_show(600.0, false, true));
    }

    #[test]
    fn wide_window_shows_a_wanted_open_panel_without_needing_an_override() {
        assert!(detail_panel_should_show(1180.0, true, false));
        assert!(detail_panel_should_show(
            DETAIL_PANEL_BREAKPOINT,
            true,
            false
        ));
    }

    #[test]
    fn narrow_window_auto_collapses_a_wanted_open_panel_with_no_override() {
        assert!(!detail_panel_should_show(820.0, true, false));
        assert!(!detail_panel_should_show(
            DETAIL_PANEL_BREAKPOINT - 1.0,
            true,
            false
        ));
    }

    #[test]
    fn explicit_narrow_reopen_is_respected_via_the_override() {
        assert!(detail_panel_should_show(820.0, true, true));
    }

    #[test]
    fn override_is_dropped_once_the_window_is_wide_enough() {
        assert!(!narrow_override_after_resize(DETAIL_PANEL_BREAKPOINT, true));
        assert!(!narrow_override_after_resize(1920.0, true));
    }

    #[test]
    fn override_persists_while_still_narrow() {
        assert!(narrow_override_after_resize(820.0, true));
        assert!(!narrow_override_after_resize(820.0, false));
    }

    #[test]
    fn width_collapsed_pane_restores_on_widen_without_an_override() {
        // The end-to-end story requirement 2 asks for: a pane that
        // auto-collapsed (no override ever set) comes back on its own once
        // the window widens past the breakpoint.
        let user_wants_open = true;
        let mut narrow_override = false;

        // Starts wide: shown.
        assert!(detail_panel_should_show(
            1920.0,
            user_wants_open,
            narrow_override
        ));
        // Narrows below the breakpoint: auto-collapses.
        narrow_override = narrow_override_after_resize(700.0, narrow_override);
        assert!(!detail_panel_should_show(
            700.0,
            user_wants_open,
            narrow_override
        ));
        // Widens back out: restores, with no click required.
        narrow_override = narrow_override_after_resize(1920.0, narrow_override);
        assert!(detail_panel_should_show(
            1920.0,
            user_wants_open,
            narrow_override
        ));
    }

    #[test]
    fn user_closing_while_narrow_does_not_reopen_on_widen() {
        // A pane the user explicitly closed must stay closed when the
        // window widens — only auto-collapsed panes restore themselves.
        assert!(!detail_panel_should_show(1920.0, false, false));
    }

    // -----------------------------------------------------------------
    // wide_tabs_fit
    // -----------------------------------------------------------------

    #[test]
    fn wide_tabs_fit_only_at_the_default_width_or_wider() {
        assert!(!wide_tabs_fit(WIDE_TABS_BREAKPOINT - 1.0));
        assert!(wide_tabs_fit(WIDE_TABS_BREAKPOINT));
        assert!(wide_tabs_fit(1920.0));
    }

    #[test]
    fn wide_tabs_do_not_fit_at_the_true_minimum_window() {
        assert!(!wide_tabs_fit(820.0));
    }

    // -----------------------------------------------------------------
    // FooterHints
    // -----------------------------------------------------------------

    #[test]
    fn footer_hints_show_everything_with_plenty_of_room() {
        assert_eq!(FooterHints::for_content_column(900.0), FooterHints::All);
    }

    #[test]
    fn footer_hints_degrade_by_tier_as_room_shrinks() {
        assert_eq!(FooterHints::for_content_column(480.0), FooterHints::All);
        assert_eq!(FooterHints::for_content_column(479.9), FooterHints::Core);
        assert_eq!(FooterHints::for_content_column(360.0), FooterHints::Core);
        assert_eq!(FooterHints::for_content_column(359.9), FooterHints::Minimal);
        assert_eq!(FooterHints::for_content_column(260.0), FooterHints::Minimal);
        assert_eq!(FooterHints::for_content_column(259.9), FooterHints::None);
        assert_eq!(FooterHints::for_content_column(0.0), FooterHints::None);
    }
}

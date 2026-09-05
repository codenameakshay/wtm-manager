//! The detail panel: everything known about the selected worktree, rendered
//! down the right edge of the window as a tabbed inspector — Details (what
//! this module always showed), Files (`crate::file_browser`), and Changes
//! (`crate::diff_view`).
//!
//! Pure rendering only, in the spirit of [`crate::worktree_list`]: this
//! module takes already-loaded [`WorktreeInfo`] and [`WorktreeDetails`]
//! values and turns them into elements. It must never call git or spawn a
//! task — that stays in [`crate::data`] and the view that owns the panel's
//! state, which is what lets `details` legitimately be `None` (still
//! loading) without this module knowing anything about *why*.
//!
//! This module only renders the Details tab's content plus the header
//! shared by every tab; the outer frame (width, background, tab bar) is
//! assembled by `crate::app::chrome`, which is also what wires the tab
//! bar's clicks and the Files tab's tree clicks — both need
//! `Context<WtmApp>`, which this module deliberately never touches. See
//! `chrome::render_detail_panel`.
//!
//! `WorktreeDetails::commits` (see `wtm::worktree::CommitLine`) only carries
//! an abbreviated commit id and its first summary line — no author or
//! timestamp — so the commit rows below show sha + subject only.

use gpui::prelude::*;
use gpui::{div, px, AnyElement, FontWeight, SharedString};
use unicode_segmentation::UnicodeSegmentation;
use wtm::model::WorktreeInfo;
use wtm::worktree::{CommitLine, WorktreeDetails};

use crate::assets::icons;
use crate::theme::{Theme, SPACE_12, SPACE_16, SPACE_6, SPACE_8};
use crate::ui;

/// Panel width when the Details tab is active — the original, unchanged
/// fixed inspector width.
pub const WIDTH: f32 = 320.0;

/// Panel width when the Files or Changes tab is active. A diff needs real
/// room: at `WIDTH` a unified diff's gutter alone would eat a third of the
/// available space. Roughly double `WIDTH` fits a file tree column plus a
/// diff wide enough for ~70-80 monospace characters before horizontal
/// scrolling kicks in (see `crate::diff_view`'s "Long lines" doc), while
/// still leaving the worktree list a usable width at the window's default
/// 1180px size — that leftover column (1180 - `SIDEBAR_WIDTH` - `WIDE_WIDTH`
/// = 292px) is what `app::layout::MIN_CONTENT_COLUMN` names and reuses as
/// the whole app's "usable content column" floor.
///
/// Below `app::layout::WIDE_TABS_BREAKPOINT` (which works out to exactly
/// 1180px, by construction — see that constant's doc), the Files/Changes
/// tabs are unreachable rather than squeezing the list: at the window's
/// 820px minimum this width alongside the sidebar leaves `820 - 248 - 640 =
/// -68px` for the list, a negative column, not merely a tight one. That's a
/// structural impossibility a click can't opt back into, unlike the
/// ordinary detail panel's own narrower, user-overridable auto-collapse
/// (`app::layout::detail_panel_should_show`).
pub const WIDE_WIDTH: f32 = 640.0;

/// Which section of the detail panel is currently shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailTab {
    #[default]
    Details,
    Files,
    Changes,
}

/// # Why fact/commit values below use a *definite* pixel width, not `flex_1`
///
/// gpui 0.2.2 has a text-measurement caching bug that makes `.truncate()`
/// silently never ellipsize inside a flex chain that gets measured more
/// than once during layout (nearly every real chain, once panels/rows nest
/// a couple of levels deep). It is not fixed in a newer release either:
/// 0.2.2 is the newest `gpui` on crates.io as of this writing, so there is
/// no upgrade path out of it.
///
/// The mechanism (`gpui-0.2.2/src/elements/text.rs`,
/// `TextLayout::layout`'s measured-layout closure): the text element caches
/// its computed size keyed on `wrap_width`, but `wrap_width` is
/// unconditionally `None` for `nowrap` text — which `.truncate()` sets via
/// `whitespace_nowrap()` — so the cache guard
/// (`wrap_width.is_none() || wrap_width == text_layout.wrap_width`) is
/// trivially true on *every* call. Taffy's flexbox algorithm measures a
/// flex child's intrinsic size at least twice: once with indefinite
/// available space (to size an ancestor that has no explicit width of its
/// own), which computes `truncate_width = None` — no truncation, full
/// content width — and caches that; then again with the real, resolved
/// available space. The second call hits the already-`Some` cache and
/// returns the *first* (untruncated) size verbatim, so the element never
/// reports narrower than its full content and `text_ellipsis()` never
/// renders its "…" — the parent's `overflow_hidden()` (part of the same
/// `.truncate()`) just hard-clips whatever doesn't fit, mid-glyph, with no
/// ellipsis at all.
///
/// The fix that actually works: give the text element a width gpui can
/// resolve on its *first* measurement, before anything gets cached wrong.
/// `known_dimensions.width` (checked ahead of `wrap_width` in the same
/// closure) is `Some(..)` immediately for an element with an explicit
/// `.w(px(..))` — no ambiguous multi-pass measurement, so the caching bug
/// never gets a chance to trigger. Every value below this fact list uses is
/// laid out at exactly one of two fixed panel widths (`WIDTH`/`WIDE_WIDTH`),
/// so the pixel budget is knowable arithmetic, not a flex unknown — see
/// `FACT_VALUE_WIDTH`/`COMMIT_SHA_WIDTH`/`COMMIT_SUBJECT_WIDTH` below.
/// `.truncate()` stays on each value as a backstop (it still *clips*
/// correctly even when it can't ellipsize) rather than as the primary
/// truncation mechanism.
///
/// Where the panel width is genuinely fluid instead of fixed (a worktree
/// row's path, a sidebar repo's path, the footer's hint line — none of
/// which live at a constant pixel width), a definite width isn't available
/// to compute, so those sites shorten the string in Rust and append `…`
/// themselves instead of leaning on `.truncate()` at all; see
/// `truncate_path_tail` below, reused by `worktree_list`/`app::chrome` for
/// exactly that.
///
/// Fixed width of a fact row's label column (`fact_row`, `skeleton_fact_row`).
/// Wide enough to fit "Ahead/Behind" — the longest label in use — on one
/// line at its `TEXT_SM` text size; see `fact_row`'s doc for why this also
/// needs `.truncate()` as a backstop rather than relying on width alone.
const LABEL_WIDTH: f32 = 88.0;

/// Definite width of a fact row's *value* column (`fact_row`), so
/// `.truncate()` actually ellipsizes instead of hitting the caching bug
/// documented on [`LABEL_WIDTH`]'s own doc above. The Details tab
/// only ever lays out at `WIDTH` (320), so this is exact arithmetic, not a
/// guess: `WIDTH` minus `render_details`'s own `SPACE_16` padding on both
/// edges, minus `LABEL_WIDTH` and its `SPACE_8` gap to the value.
const FACT_VALUE_WIDTH: f32 = WIDTH - SPACE_16 * 2.0 - LABEL_WIDTH - SPACE_8;

/// How many characters fit inside [`FACT_VALUE_WIDTH`] — `ui::CHAR_WIDTH_APPROX`
/// px/character (gpui has no API to measure real shaped text outside of an
/// actual layout pass) over 192px is ≈26.7 characters, rounded down one
/// further for margin. Used two ways: `truncate_path_tail` applies it to
/// Path specifically, which wants a *leading* ellipsis so the readable tail
/// of the path survives; `fact_row` applies it (via `truncate_tail`,
/// trailing ellipsis) to every other value, since — per `LABEL_WIDTH`'s
/// doc — gpui's own `.truncate()`/`text_ellipsis()` does not reliably
/// render an ellipsis glyph even once given a definite width to measure
/// against, only `LABEL_WIDTH`-style upfront clipping. Both stay a *little*
/// conservative on purpose (rounded down, not to the nearest fit) because
/// this is an approximation, not a measurement — better to end one
/// character short of the edge than one character over it.
const FACT_VALUE_MAX_CHARS: usize = 25;

/// Fixed width of a commit row's sha column (`render_commit_row`) — see
/// `LABEL_WIDTH`'s doc for why this needs to be definite at all.
/// Generous for a typical abbreviated sha (`short_id` usually lands on 7
/// hex characters; this covers up to ~11 at `FONT_MONO`/`TEXT_XS` before
/// its own `.truncate()` backstop would need to act) rather than exactly
/// fitted, since — unlike the label column — a slightly-too-narrow sha
/// column has no `LABEL_WIDTH`-style precedent value to size against.
const COMMIT_SHA_WIDTH: f32 = 72.0;

/// Definite width of a commit row's subject column, computed the same way
/// as [`FACT_VALUE_WIDTH`]: `WIDTH` minus `render_details`'s `SPACE_16`
/// padding on both edges, minus [`COMMIT_SHA_WIDTH`] and its `SPACE_8` gap
/// to the subject.
const COMMIT_SUBJECT_WIDTH: f32 = WIDTH - SPACE_16 * 2.0 - COMMIT_SHA_WIDTH - SPACE_8;

/// How many characters fit inside [`COMMIT_SUBJECT_WIDTH`] — same
/// derivation and same margin-of-one conservatism as
/// [`FACT_VALUE_MAX_CHARS`], just against 208px instead of 192px (≈28.9,
/// rounded down to 27, minus 1 more for margin).
const COMMIT_SUBJECT_MAX_CHARS: usize = 26;

/// The Details tab's content: the fact list, status pills, and recent
/// commits. `chrome::render_detail_panel` supplies the outer frame and tab
/// bar around it.
pub fn render_details(
    info: &WorktreeInfo,
    details: Option<&WorktreeDetails>,
    theme: &Theme,
) -> impl IntoElement {
    // The three sections below (facts, status, commits) are separate
    // groups, so their gap is `SPACE_16` — double each section's own
    // internal `SPACE_8` gap between its own rows, per `better-layout` §1
    // ("the gap between groups must be at least 2x the gap within a
    // group"), rather than per-row borders.
    div()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .flex()
        .flex_col()
        .gap(px(SPACE_16))
        .px(px(SPACE_16))
        .py(px(SPACE_12))
        .child(render_facts(info, details, theme))
        .child(render_status(info, theme))
        .child(render_commits(details, theme))
}

/// Branch name, the `main` badge, and a lock indicator — the same badges
/// `worktree_list` shows on a row, so recognizing the selected worktree in
/// the panel takes no relearning. Shown above the tab bar regardless of
/// which tab is active, so the user always knows whose files/changes
/// they're looking at.
pub fn render_header(info: &WorktreeInfo, theme: &Theme) -> impl IntoElement {
    div()
        .w_full()
        .min_w_0()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(SPACE_8))
        .px(px(SPACE_16))
        .py(px(SPACE_12))
        .border_b_1()
        .border_color(theme.border)
        .child(
            // Branch at `TEXT_MD`/600 — one step heavier than the list
            // row's `TEXT_BASE`/500, since this is the one place the panel
            // states its subject. `.id(..)` because `.tooltip(..)` is
            // `StatefulInteractiveElement`-only (gpui 0.2.2).
            div()
                .id("detail-header-name")
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(ui::TEXT_MD))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(info.display_name().to_string())
                .tooltip(ui::tooltip(info.display_name().to_string())),
        )
        .when(info.is_main, |this| this.child(ui::main_badge(theme)))
        .when(info.is_locked, |this| {
            this.child(ui::icon(icons::LOCK, 11.0, theme.text_ghost))
        })
}

/// The definition list of facts about the worktree. Path/HEAD/ahead-behind
/// come straight from `info`, which the caller always has; upstream and
/// remote come from `details` and show a skeleton line until it arrives.
fn render_facts(
    info: &WorktreeInfo,
    details: Option<&WorktreeDetails>,
    theme: &Theme,
) -> impl IntoElement {
    let full_path = info.path.display().to_string();
    div()
        .w_full()
        .min_w_0()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(SPACE_8))
        .child(fact_row(
            "Path",
            // Truncates from the *start*, keeping the readable tail. The
            // panel-width-limited string above is what paints; `full_path`
            // (moved into the tooltip below, since nothing else needs it
            // after this) is what the tooltip shows.
            truncate_path_tail(&full_path, FACT_VALUE_MAX_CHARS),
            true,
            Some(full_path.into()),
            theme,
        ))
        .child(fact_row(
            "HEAD",
            info.head.clone().unwrap_or_else(|| "-".to_string()),
            true,
            None,
            theme,
        ))
        .child(render_detail_row("Upstream", details, theme, |d| {
            d.upstream.clone().unwrap_or_else(|| "-".to_string())
        }))
        .child(render_detail_row("Remote", details, theme, |d| {
            remote_name(d.upstream.as_deref())
                .unwrap_or("-")
                .to_string()
        }))
        .child(render_ahead_behind_row(info, theme))
}

/// One `Upstream`/`Remote`-shaped fact row: a skeleton while `details` is
/// still loading, otherwise `extract`'s value through [`fact_row`].
fn render_detail_row(
    label: &'static str,
    details: Option<&WorktreeDetails>,
    theme: &Theme,
    extract: impl Fn(&WorktreeDetails) -> String,
) -> AnyElement {
    match details {
        None => skeleton_fact_row(label, theme).into_any_element(),
        Some(details) => fact_row(label, extract(details), true, None, theme).into_any_element(),
    }
}

fn render_ahead_behind_row(info: &WorktreeInfo, theme: &Theme) -> impl IntoElement {
    let value = match &info.status {
        Some(status) if status.ahead.is_none() && status.behind.is_none() => {
            "no upstream".to_string()
        }
        Some(status) => format!(
            "{} ahead · {} behind",
            status.ahead.unwrap_or(0),
            status.behind.unwrap_or(0)
        ),
        None => "-".to_string(),
    };
    // Not a path/sha/ref — a natural-language phrase — so no mono, no
    // tooltip.
    fact_row("Ahead/Behind", value, false, None, theme)
}

/// One label/value line: a `TEXT_SM` `text_muted` label, a `TEXT_SM`
/// `text`-colored value. A definition list's actual information is the
/// value; it should read at full weight while its label recedes.
///
/// `mono` renders the value in [`ui::FONT_MONO`] — every value here that is
/// a path, sha, or ref name sets it. `tooltip`, when given, carries the
/// value's untruncated text.
fn fact_row(
    label: &'static str,
    value: impl Into<SharedString>,
    mono: bool,
    tooltip: Option<SharedString>,
    theme: &Theme,
) -> impl IntoElement {
    let value: SharedString = value.into();
    // `truncate_tail` (trailing ellipsis, computed in Rust) is the real
    // truncation authority here, not gpui's own `.truncate()` — see
    // `LABEL_WIDTH`'s doc for why. A no-op for a value `truncate_path_tail`
    // (Path's own caller-side, leading-ellipsis shortening) already brought
    // under budget, so this is safe to apply unconditionally to every
    // value, not just the ones that need it.
    let shown = truncate_tail(&value, FACT_VALUE_MAX_CHARS);
    // Every value gets a way to read its full text: the caller's own
    // tooltip when it passed one (Path already does), otherwise whatever
    // `truncate_tail` actually shortened.
    let tooltip = tooltip.or_else(|| (shown != value).then(|| value.clone()));
    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_baseline()
        .gap(px(SPACE_8))
        .child(
            // Fixed width plus `truncate()` (which implies no-wrap): wide
            // enough for every label used today, but a label landing right
            // at the edge (e.g. "Ahead/Behind") must never wrap onto a
            // second line rather than simply clipping — see
            // `render_ahead_behind_row`.
            div()
                .flex_none()
                .w(px(LABEL_WIDTH))
                .truncate()
                .text_size(px(ui::TEXT_SM))
                .text_color(theme.text_muted)
                .child(label),
        )
        .child(
            // `.id(label)` (each call site's `label` is a distinct static
            // string) so `.tooltip(..)` — `StatefulInteractiveElement`-only
            // in gpui 0.2.2 — is available regardless of whether this
            // particular row has one. `FACT_VALUE_WIDTH` (a *definite*
            // width, not `flex_1`/`min_w_0`) plus `.truncate()` here is
            // only a defensive backstop — `shown` is already short enough
            // to fit, since gpui's own ellipsis glyph does not reliably
            // render even given a definite width to measure against (see
            // `LABEL_WIDTH`'s doc).
            div()
                .id(label)
                .flex_none()
                .w(px(FACT_VALUE_WIDTH))
                .truncate()
                .when(mono, |d| d.font_family(ui::FONT_MONO))
                .text_size(px(ui::TEXT_SM))
                .text_color(theme.text)
                .child(shown)
                .when_some(tooltip, |d, text| d.tooltip(ui::tooltip(text))),
        )
}

/// A muted placeholder bar in place of a fact whose value is still loading,
/// so the panel never shows a blank gap where an upstream or remote will be.
fn skeleton_fact_row(label: &'static str, theme: &Theme) -> impl IntoElement {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(SPACE_8))
        .child(
            div()
                .flex_none()
                .w(px(LABEL_WIDTH))
                .truncate()
                .text_size(px(ui::TEXT_SM))
                .text_color(theme.text_muted)
                .child(label),
        )
        // Same 88x10 placeholder size this bar always had — just routed
        // through `ui::skeleton` now instead of a hand-rolled `item_wash`
        // div.
        .child(ui::skeleton(88.0, 10.0, theme))
}

/// Status pills, in the same order and with the same vocabulary as
/// `worktree_list`'s row pills, so the list and the panel never disagree
/// about what a badge means. Gets its own room around it via the
/// `SPACE_16` gap `render_details` puts between this and its neighbor
/// sections — double this row's own `SPACE_8` pill-to-pill gap.
fn render_status(info: &WorktreeInfo, theme: &Theme) -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .flex_wrap()
        .gap(px(SPACE_8))
        .children(status_pills(info, theme))
}

fn status_pills(info: &WorktreeInfo, theme: &Theme) -> Vec<AnyElement> {
    if info.is_missing {
        return vec![ui::pill("missing", theme.danger).into_any_element()];
    }

    let Some(status) = &info.status else {
        return vec![div()
            .text_size(px(ui::TEXT_SM))
            .text_color(theme.text_ghost)
            .child("status unknown")
            .into_any_element()];
    };

    crate::worktree_list::status_pill_specs(status, theme)
        .into_iter()
        .map(|spec| match spec.color {
            Some(color) => ui::pill(spec.label, color).into_any_element(),
            None => div()
                .text_size(px(ui::TEXT_SM))
                .text_color(theme.text_ghost)
                .child(spec.label)
                .into_any_element(),
        })
        .collect()
}

/// A skeleton while `details` loads, an honest empty state when the
/// worktree genuinely has no commits, otherwise compact sha/subject rows —
/// no "Recent commits" eyebrow above it. A list of shas and subjects
/// already announces what it is; the `SPACE_16` gap `render_details` puts
/// ahead of this section (double this section's own internal `SPACE_8`/
/// `SPACE_6` row gap) is what tells it apart from the facts/status groups
/// above, per `better-layout` §1 — a muted label restating "these are
/// commits" on top of that spacing was grammar nobody chose.
fn render_commits(details: Option<&WorktreeDetails>, theme: &Theme) -> impl IntoElement {
    div()
        .w_full()
        .min_w_0()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(SPACE_8))
        .child(match details {
            None => render_commit_skeleton(theme).into_any_element(),
            Some(details) if details.commits.is_empty() => {
                render_no_commits(theme).into_any_element()
            }
            Some(details) => render_commit_list(&details.commits, theme).into_any_element(),
        })
}

fn render_commit_skeleton(theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_8))
        .children((0..4).map(|_| {
            // Width is irrelevant here — `.w_full()` overrides `ui::skeleton`'s
            // fixed-width default immediately after.
            ui::skeleton(1.0, 11.0, theme).w_full()
        }))
}

fn render_no_commits(theme: &Theme) -> impl IntoElement {
    div()
        .text_size(px(ui::TEXT_SM))
        .text_color(theme.text_ghost)
        .child("No commits yet")
}

fn render_commit_list(commits: &[CommitLine], theme: &Theme) -> impl IntoElement {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(SPACE_6))
        .children(
            commits
                .iter()
                .map(|commit| render_commit_row(commit, theme)),
        )
}

/// One compact commit row: sha in `FONT_MONO`/`text_ghost`, subject in full
/// `text` weight, truncated to a single line with the untruncated subject in
/// a tooltip. `CommitLine` carries no author or timestamp (see the module
/// doc comment), so those columns are intentionally absent rather than
/// faked.
fn render_commit_row(commit: &CommitLine, theme: &Theme) -> impl IntoElement {
    let subject = if commit.summary.is_empty() {
        "(no message)".to_string()
    } else {
        commit.summary.clone()
    };
    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(SPACE_8))
        .child(
            // `COMMIT_SHA_WIDTH` is a definite width, same reason as
            // `COMMIT_SUBJECT_WIDTH` below — see `LABEL_WIDTH`'s doc.
            div()
                .flex_none()
                .w(px(COMMIT_SHA_WIDTH))
                .truncate()
                .font_family(ui::FONT_MONO)
                .text_size(px(ui::TEXT_XS))
                .text_color(theme.text_ghost)
                .child(commit.id.clone()),
        )
        .child(
            // `.id(..)` (keyed on the commit's own sha, unique per row) so
            // `.tooltip(..)` — `StatefulInteractiveElement`-only in gpui
            // 0.2.2 — is available on this div. `truncate_tail` is the real
            // truncation authority (see `LABEL_WIDTH`'s doc); the tooltip
            // always carries the untruncated `subject` regardless.
            div()
                .id(SharedString::from(format!("commit-subject:{}", commit.id)))
                .flex_none()
                .w(px(COMMIT_SUBJECT_WIDTH))
                .truncate()
                .text_size(px(ui::TEXT_SM))
                .text_color(theme.text)
                .child(truncate_tail(&subject, COMMIT_SUBJECT_MAX_CHARS))
                .tooltip(ui::tooltip(subject)),
        )
}

/// The remote name is the segment before the first `/` in the upstream
/// shorthand (`"origin/main"` -> `"origin"`). `WorktreeDetails` has no
/// separate remote field, so this is derived rather than duplicated data.
fn remote_name(upstream: Option<&str>) -> Option<&str> {
    upstream.and_then(|u| u.split_once('/').map(|(remote, _)| remote))
}

/// Condense a long path to fit a target width, keeping the readable tail —
/// the worktree's own directory name matters far more than a prefix shared
/// by every worktree of the repo. `pub(crate)` so `worktree_list`/
/// `app::chrome` can reuse it for the other genuinely-fluid-width paths in
/// the app (a worktree row's path, a sidebar repo's path) instead of each
/// hand-rolling the same leading-ellipsis logic — see `LABEL_WIDTH`'s doc
/// for why gpui's own `.truncate()` isn't what does this job.
///
/// Counts and slices by **extended grapheme cluster**
/// ([`UnicodeSegmentation::graphemes`]), not `char`: git permits combining
/// accents and emoji (including multi-scalar ones) in paths, and slicing by
/// `char` — while never a panic, `char` boundaries are always valid UTF-8
/// slice points — can still cut a base character apart from its combining
/// mark, or split a multi-codepoint emoji, leaving a broken glyph on
/// screen. Grapheme clusters are the smallest unit that's always safe to
/// cut between.
pub(crate) fn truncate_path_tail(path: &str, max_chars: usize) -> String {
    let len = path.graphemes(true).count();
    if len <= max_chars {
        return path.to_string();
    }
    let tail_len = max_chars.saturating_sub(1); // room for the leading ellipsis
    let tail: String = path.graphemes(true).skip(len - tail_len).collect();
    format!("…{tail}")
}

/// Trailing-ellipsis counterpart to [`truncate_path_tail`]: keeps the
/// *start* of `s` and ellipsizes the end, instead of the other way around.
/// This is what every non-path value wants — a branch/ref name or a commit
/// subject is read from the front, so losing the end (not the beginning)
/// is what keeps the readable part on screen. See `LABEL_WIDTH`'s doc for
/// why this exists at all instead of gpui's own `.truncate()`.
///
/// `pub(crate)` (rather than private) so `worktree_list::render_row` can
/// reuse it for the branch name — same trailing-ellipsis shape, same reason
/// — instead of a second copy of this exact logic.
///
/// Grapheme-cluster-safe, for the same reason [`truncate_path_tail`] is —
/// see its doc.
pub(crate) fn truncate_tail(s: &str, max_chars: usize) -> String {
    let len = s.graphemes(true).count();
    if len <= max_chars {
        return s.to_string();
    }
    let head_len = max_chars.saturating_sub(1); // room for the trailing ellipsis
    let head: String = s.graphemes(true).take(head_len).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_path_is_unchanged() {
        assert_eq!(truncate_path_tail("/tmp/repo", 40), "/tmp/repo");
    }

    #[test]
    fn long_path_keeps_the_tail_and_fits_the_budget() {
        let path = "/Users/example/code/very/deeply/nested/worktrees/feature-branch";
        let out = truncate_path_tail(path, 24);
        assert_eq!(out.chars().count(), 24);
        assert!(out.starts_with('…'));
        assert!(out.ends_with("feature-branch"));
    }

    // -------------------------------------------------------------
    // Unicode edge cases (git permits emoji, CJK, and combining accents in
    // branch names and paths) — the truncation helpers must never panic on
    // a non-char-boundary slice, and must never split an extended grapheme
    // cluster (a base character plus its combining mark, or a multi-scalar
    // emoji) across the truncation point.
    // -------------------------------------------------------------

    #[test]
    fn truncate_tail_does_not_panic_or_split_a_wide_emoji_branch_name() {
        // Each of these is one grapheme cluster: a plain emoji, a
        // multi-codepoint family emoji (ZWJ sequence), and a flag (regional
        // indicator pair) — none of these are single `char`s, so a naive
        // byte-index or char-index cut through the middle would either
        // panic (byte) or emit a broken/partial glyph (char).
        let branch = "feature/🔥🔥🔥🔥🔥-launch-👨‍👩‍👧‍👦-🇯🇵-rollout";
        for budget in 0..=branch.chars().count() {
            let out = truncate_tail(branch, budget);
            // Must always be valid UTF-8 (guaranteed by the type) and must
            // never contain fewer grapheme clusters than it claims to.
            assert!(out.graphemes(true).count() <= budget.max(1));
            // Never emits a lone combining/joiner artifact: every grapheme
            // in the output is one of the source's own intact clusters, or
            // the ellipsis we added ourselves.
            for g in out.graphemes(true) {
                assert!(g == "…" || branch.graphemes(true).any(|src| src == g));
            }
        }
    }

    #[test]
    fn truncate_tail_does_not_panic_on_cjk_branch_names() {
        // Every CJK ideograph below is one grapheme cluster and 3 bytes in
        // UTF-8 — a byte-index cut anywhere but a multiple of 3 would panic
        // on this input under the old `.chars()`-unaware approach this
        // guards against regressing to.
        let branch = "功能/日本語のブランチ名はとても長くなることがあります";
        for budget in 0..=branch.chars().count() + 2 {
            let out = truncate_tail(branch, budget);
            assert!(out.graphemes(true).count() <= budget.max(1));
        }
    }

    #[test]
    fn truncate_tail_keeps_combining_accents_attached_to_their_base_char() {
        // "e" + U+0301 COMBINING ACUTE ACCENT is two `char`s but one
        // grapheme cluster ("é"). A `.chars()`-based truncation could stop
        // between them, leaving a bare accent mark on screen.
        let branch = "cafe\u{301}-e\u{301}toile\u{301}-branch";
        let cluster_count = branch.graphemes(true).count();
        for budget in 0..=cluster_count + 2 {
            let out = truncate_tail(branch, budget);
            // Every grapheme in the truncated output must be a complete,
            // intact cluster from the source (or the ellipsis) — never a
            // bare base character or a bare combining mark.
            for g in out.graphemes(true) {
                assert!(
                    g == "…" || branch.graphemes(true).any(|src| src == g),
                    "grapheme {g:?} in output was not an intact cluster from {branch:?}"
                );
            }
        }
    }

    #[test]
    fn truncate_path_tail_keeps_combining_accents_attached_at_the_leading_edge() {
        // Same guarantee as `truncate_tail`'s combining-accent case, but for
        // the leading-ellipsis path variant, which slices from the front
        // instead of the back.
        let path = "/répertoire/e\u{301}toile\u{301}/projet";
        let cluster_count = path.graphemes(true).count();
        for budget in 0..=cluster_count + 2 {
            let out = truncate_path_tail(path, budget);
            for g in out.graphemes(true) {
                assert!(
                    g == "…" || path.graphemes(true).any(|src| src == g),
                    "grapheme {g:?} in output was not an intact cluster from {path:?}"
                );
            }
        }
    }

    #[test]
    fn truncate_tail_handles_a_very_long_branch_name() {
        // 100+ chars: a pathological but real branch name (e.g. a
        // ticket-system-generated slug).
        let branch = "feature/".to_string() + &"a".repeat(150);
        let out = truncate_tail(&branch, 24);
        assert_eq!(out.chars().count(), 24);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_tail_handles_a_one_character_branch_name() {
        assert_eq!(truncate_tail("a", 24), "a");
        assert_eq!(truncate_tail("a", 0), "…");
        assert_eq!(truncate_tail("🔥", 0), "…");
    }

    #[test]
    fn truncate_path_tail_handles_a_one_character_path() {
        assert_eq!(truncate_path_tail("a", 24), "a");
        assert_eq!(truncate_path_tail("a", 0), "…");
    }

    #[test]
    fn remote_name_splits_on_first_slash() {
        assert_eq!(remote_name(Some("origin/main")), Some("origin"));
        assert_eq!(remote_name(Some("origin/feature/x")), Some("origin"));
        assert_eq!(remote_name(None), None);
    }
}

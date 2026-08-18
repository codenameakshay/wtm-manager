//! Inline rendering of a [`FileDiff`]: unified-diff hunks with a line-number
//! gutter, the same visual language `git diff` output uses, drawn straight
//! into the detail panel instead of shelling out to a pager.
//!
//! Everything here is pure rendering over already-loaded data — no git, no
//! background spawn, no `Context<WtmApp>` — in the same spirit as
//! [`crate::detail_panel`] and [`crate::file_browser`]'s tree rendering.
//! Nothing in a diff is clickable today, so unlike `file_browser::render_row`
//! there is no click handler for a caller to attach; `render_diff`/
//! `render_changes` are ready to drop straight into an element tree.
//!
//! ## Font
//!
//! The diff body uses [`ui::FONT_MONO`] (Geist Mono, bundled — SPEC §6: diff
//! content is one of the things that gets the app's own mono face, the same
//! one paths/shas/branch names use in meta position). Pre-redesign this used
//! the platform's own "SF Mono", falling through "Menlo"/"Monaco"/"Courier
//! New" — those four now sit *after* the bundled face in the fallback list
//! (see [`MONOSPACE_FALLBACKS`]) rather than being the primary, so a
//! diff still renders in something reasonably monospace on the rare path
//! where font registration fails (SPEC §6: that failure is non-fatal).
//! `gpui`'s font resolution falls through this list if an earlier name
//! doesn't resolve in the current font context, rather than silently
//! substituting the UI's proportional face for code.
//!
//! ## Long lines
//!
//! A diff line is rendered `whitespace_nowrap()` with no `min_w_0()`/
//! `truncate()` — unlike nearly every other row in this app — so its
//! intrinsic content width can exceed its row's flex-computed width. Rows
//! sit inside a `.overflow_x_scroll()` container, so a long line scrolls
//! into view horizontally instead of wrapping (which would make the gutter's
//! line numbers stop lining up with their own line) or getting silently
//! clipped (which would hide real content). Vertical scrolling is left to
//! the caller (`crate::app::chrome`), which wraps the whole tab in one
//! `.overflow_y_scroll()` — the Changes tab needs that to span every file's
//! diff, not just one.

use gpui::prelude::*;
use gpui::{div, font, px, AnyElement, Font, FontFallbacks, Hsla, SharedString};

use crate::data::{DiffHunk, DiffLine, DiffLineKind, FileDiff};
use crate::file_browser::{status_color, status_label};
use crate::theme::{Theme, RADIUS_CONTROL, SPACE_16, SPACE_24, SPACE_4, SPACE_8};
use crate::ui;

/// Platform monospace fallbacks, tried in order after [`ui::FONT_MONO`] (see
/// the module doc's "Font" section) if the bundled face fails to register.
const MONOSPACE_FALLBACKS: &[&str] = &["SF Mono", "Menlo", "Monaco", "Courier New"];

/// Estimated advance width, in pixels, of one monospace character at the
/// diff body's `TEXT_SM` text size — used only to size the line-number
/// gutter (see [`gutter_width`]); gpui has no API to measure real shaped
/// text outside of an actual layout pass, so this is a deliberate
/// approximation (roughly 0.6em, typical for a monospace face — including
/// Geist Mono) rather than an exact value.
const GUTTER_CHAR_WIDTH: f32 = 7.2;
/// Horizontal padding inside a gutter column, both sides combined.
const GUTTER_PADDING: f32 = 10.0;

fn diff_font() -> Font {
    let mut f = font(ui::FONT_MONO);
    f.fallbacks = Some(FontFallbacks::from_fonts(
        MONOSPACE_FALLBACKS.iter().map(|s| s.to_string()).collect(),
    ));
    f
}

/// Number of characters needed to print the largest line number appearing
/// anywhere in `diff`'s hunks — so a 9-line file's gutter isn't as wide as a
/// 12,000-line file's, and a 12,000-line file's numbers don't get clipped by
/// a gutter sized for 3 digits. `1` for a diff with no line numbers at all
/// (e.g. no hunks), so [`gutter_width`] never computes a zero-width column.
pub fn gutter_digits(diff: &FileDiff) -> usize {
    let max = diff
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .flat_map(|l| [l.old_lineno, l.new_lineno])
        .flatten()
        .max()
        .unwrap_or(1);
    max.to_string().len().max(1)
}

/// Pixel width of one gutter column sized for `digits` characters — see
/// [`GUTTER_CHAR_WIDTH`]'s doc for why this is an estimate, not a
/// measurement.
pub fn gutter_width(digits: usize) -> f32 {
    GUTTER_PADDING + digits as f32 * GUTTER_CHAR_WIDTH
}

/// The `+`/`-`/` ` origin marker `git diff` prints before each line. Kept in
/// its own fixed column in the gutter rather than prefixed onto the line's
/// own text, so a line that itself starts with `+` or `-` (a shell script, a
/// diff-of-a-diff) can never be confused with the marker.
pub fn line_marker(kind: DiffLineKind) -> &'static str {
    match kind {
        DiffLineKind::Added => "+",
        DiffLineKind::Removed => "-",
        DiffLineKind::Context => " ",
    }
}

/// Text for one gutter cell: the line number, or blank when this side of the
/// diff has none (an added line has no old number; a removed line has no
/// new one).
pub fn lineno_cell(n: Option<u32>) -> String {
    n.map(|n| n.to_string()).unwrap_or_default()
}

/// Background wash for a line's kind — `theme.diff_add_wash`/
/// `diff_del_wash` (SPEC §3's tuned per-appearance alphas), not an ad-hoc
/// tint: washing the *background* and tinting only the `+`/`-` marker
/// ([`marker_color`]) is what keeps a long diff's body text readable
/// (SURFACES §4 — "do not color the whole line's text"). `None` for context
/// lines — they get no tint at all, not even a neutral one, so the eye
/// lands on what changed.
fn line_tint(kind: DiffLineKind, theme: &Theme) -> Option<Hsla> {
    match kind {
        DiffLineKind::Added => Some(theme.diff_add_wash),
        DiffLineKind::Removed => Some(theme.diff_del_wash),
        DiffLineKind::Context => None,
    }
}

fn marker_color(kind: DiffLineKind, theme: &Theme) -> Hsla {
    match kind {
        DiffLineKind::Added => theme.diff_add,
        DiffLineKind::Removed => theme.diff_del,
        DiffLineKind::Context => theme.text_ghost,
    }
}

/// One file's diff: a header (path, status pill, binary/truncated notices)
/// followed by its hunks, or an honest explanation in place of hunks when
/// there is nothing to show (binary content, or a change with no textual
/// hunks at all — e.g. a bare mode change).
pub fn render_diff(diff: &FileDiff, theme: &Theme) -> AnyElement {
    let body: AnyElement = if diff.binary {
        empty_note("Binary file, no preview", theme)
    } else if diff.hunks.is_empty() {
        empty_note("No line changes to show for this file", theme)
    } else {
        render_hunks(diff, theme)
    };

    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(px(SPACE_8))
        .w_full()
        .child(render_header(diff, theme))
        .when(diff.truncated, |d| d.child(truncated_banner(theme)))
        .child(body)
        .into_any_element()
}

/// The Changes tab's data, loaded in the background by `crate::app::loading`
/// and kept in `WtmApp::changes`, guarded by `details_generation` the same
/// way `WtmApp::details` already is.
pub enum ChangesState {
    Loading,
    Loaded(Vec<FileDiff>),
    Error(String),
}

/// The Changes tab: every changed file's diff stacked in one column, or "No
/// uncommitted changes" in place of a blank panel when the worktree is
/// clean — an empty list is a fact worth stating, not a rendering no-op.
pub fn render_changes(diffs: &[FileDiff], theme: &Theme) -> AnyElement {
    if diffs.is_empty() {
        return empty_note("No uncommitted changes", theme);
    }
    // Between-file gap (`SPACE_24`) vs. a header/hunks within-file gap of
    // `SPACE_8` — a `better-layout` §1 group-vs-within ratio, same as
    // `detail_panel::render_details`.
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(px(SPACE_24))
        .w_full()
        .children(diffs.iter().map(|diff| render_diff(diff, theme)))
        .into_any_element()
}

fn render_header(diff: &FileDiff, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .min_w_0()
        .items_center()
        .justify_between()
        .gap(px(SPACE_8))
        .child(
            // `.id(..)` (keyed on the file's own path, unique per diff) so
            // `.tooltip(..)` — `StatefulInteractiveElement`-only in gpui
            // 0.2.2 — is available on this div.
            div()
                .id(SharedString::from(format!(
                    "diff-header-path:{}",
                    diff.path
                )))
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(ui::TEXT_SM))
                .text_color(theme.text)
                .child(diff.path.clone())
                .tooltip(ui::tooltip(diff.path.clone())),
        )
        .child(ui::pill(
            status_label(diff.status),
            status_color(diff.status, theme),
        ))
}

fn truncated_banner(theme: &Theme) -> impl IntoElement {
    div()
        .w_full()
        .px(px(SPACE_8))
        .py(px(SPACE_4))
        .rounded(px(RADIUS_CONTROL))
        // No general-purpose "warning wash" token exists in `theme.rs` yet
        // (only the diff-specific `diff_add_wash`/`diff_del_wash`) — this
        // keeps the same 0.10 alpha those use, for consistency, rather than
        // the previous ad-hoc 0.14. Worth promoting to a real
        // `warning_wash`-style token in a later phase.
        .bg(Hsla {
            a: 0.10,
            ..theme.warning
        })
        .text_size(px(ui::TEXT_XS))
        .text_color(theme.warning)
        .child("Diff truncated at 2,000 lines for this file — later changes in it aren't shown.")
}

fn empty_note(text: &'static str, theme: &Theme) -> AnyElement {
    div()
        .w_full()
        .py(px(SPACE_24))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(ui::TEXT_SM))
        .text_color(theme.text_faint)
        .child(text)
        .into_any_element()
}

fn render_hunks(diff: &FileDiff, theme: &Theme) -> AnyElement {
    let gutter_px = gutter_width(gutter_digits(diff));
    // Scoped to this file's own path: the Changes tab stacks one of these
    // per changed file, and each needs a distinct element id to scroll
    // independently of the others.
    let id = SharedString::from(format!("diff-body:{}", diff.path));
    div()
        .id(id)
        // Radius arithmetic (SPEC §4/COMPONENTS.md): the diff body is a
        // self-contained bordered block at `RADIUS_CONTROL` (6) — the
        // nearest token to the pre-redesign literal `6.0`, so this was
        // already on-scale.
        .rounded(px(RADIUS_CONTROL))
        .flex()
        .flex_col()
        .w_full()
        .border_1()
        .border_color(theme.border)
        .overflow_x_scroll()
        .font(diff_font())
        .children(
            diff.hunks
                .iter()
                .map(|hunk| render_hunk(hunk, gutter_px, theme)),
        )
        .into_any_element()
}

fn render_hunk(hunk: &DiffHunk, gutter_px: f32, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .child(
            // Hunk headers get `diff_hunk_bg` (SPEC §3/SURFACES §4) — a
            // dedicated token, not the generic hover wash (`item_wash`) this
            // used before.
            div()
                .w_full()
                .px(px(SPACE_8))
                .py(px(SPACE_4))
                .bg(theme.diff_hunk_bg)
                .whitespace_nowrap()
                .text_size(px(ui::TEXT_XS))
                .text_color(theme.text_faint)
                .child(hunk.header.clone()),
        )
        .children(
            hunk.lines
                .iter()
                .map(|line| render_line(line, gutter_px, theme)),
        )
}

/// Fixed width of the `+`/`-`/` ` marker column — sized for one monospace
/// glyph at the diff body's `TEXT_SM` text size, the same "named because it
/// doesn't fit the `SPACE_*` scale" precedent as [`GUTTER_CHAR_WIDTH`].
const MARKER_COLUMN_WIDTH: f32 = 14.0;

fn render_line(line: &DiffLine, gutter_px: f32, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .w_full()
        // Wash the line's *background*; the marker color below is the only
        // thing that carries the added/removed tint into the text itself
        // (SURFACES §4 — coloring the whole line's text makes a long diff
        // unreadable).
        .when_some(line_tint(line.kind, theme), |d, c| d.bg(c))
        .child(lineno_col(line.old_lineno, gutter_px, theme))
        .child(lineno_col(line.new_lineno, gutter_px, theme))
        .child(
            div()
                .flex_none()
                .w(px(MARKER_COLUMN_WIDTH))
                .text_size(px(ui::TEXT_SM))
                .text_color(marker_color(line.kind, theme))
                .child(line_marker(line.kind)),
        )
        .child(
            // Deliberately no `min_w_0()`/`truncate()` here — see the module
            // doc's "Long lines" section for why that's what lets this row
            // overflow into the ancestor's horizontal scroll instead of
            // wrapping or clipping. Body text always stays `theme.text`
            // regardless of `line.kind` — see `line_tint`'s doc for why the
            // background wash (not the text) is what signals a change.
            div()
                .flex_1()
                .whitespace_nowrap()
                .pr(px(SPACE_16))
                .text_size(px(ui::TEXT_SM))
                .text_color(theme.text)
                .child(if line.text.is_empty() {
                    " ".to_string()
                } else {
                    line.text.clone()
                }),
        )
}

fn lineno_col(n: Option<u32>, width: f32, theme: &Theme) -> impl IntoElement {
    div()
        .flex_none()
        .w(px(width))
        .px(px(SPACE_4))
        // Right-aligned so a run of different-width line numbers still
        // lines up on their trailing digit (SURFACES §4: "digits aligned"),
        // rather than only sharing a left edge.
        .text_right()
        .text_size(px(ui::TEXT_XS))
        .text_color(theme.text_ghost)
        .child(lineno_cell(n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::FileStatus;

    fn line(kind: DiffLineKind, old: Option<u32>, new: Option<u32>) -> DiffLine {
        DiffLine {
            kind,
            text: "x".to_string(),
            old_lineno: old,
            new_lineno: new,
        }
    }

    fn diff_with_lines(lines: Vec<DiffLine>) -> FileDiff {
        FileDiff {
            path: "f.txt".to_string(),
            status: FileStatus::Modified,
            hunks: vec![DiffHunk {
                header: "@@ -1,1 +1,1 @@".to_string(),
                lines,
            }],
            binary: false,
            truncated: false,
        }
    }

    // ---------------- gutter_digits / gutter_width ----------------

    #[test]
    fn gutter_digits_defaults_to_one_with_no_line_numbers() {
        let diff = diff_with_lines(vec![]);
        assert_eq!(gutter_digits(&diff), 1);
    }

    #[test]
    fn gutter_digits_tracks_the_largest_line_number_either_side() {
        let diff = diff_with_lines(vec![
            line(DiffLineKind::Context, Some(8), Some(8)),
            line(DiffLineKind::Added, None, Some(42)),
        ]);
        assert_eq!(gutter_digits(&diff), 2);
    }

    #[test]
    fn gutter_digits_handles_five_digit_files() {
        let diff = diff_with_lines(vec![line(DiffLineKind::Context, Some(12345), Some(12345))]);
        assert_eq!(gutter_digits(&diff), 5);
    }

    #[test]
    fn gutter_width_grows_linearly_with_digits() {
        let one = gutter_width(1);
        let two = gutter_width(2);
        let five = gutter_width(5);
        assert!(one < two);
        assert!(two < five);
        // `f32` arithmetic, not exact — compare within a tolerance rather
        // than bit-for-bit equality.
        assert!((five - two - 3.0 * (two - one)).abs() < 0.001);
    }

    // ---------------- line_marker / lineno_cell ----------------

    #[test]
    fn line_marker_matches_git_diff_origin_characters() {
        assert_eq!(line_marker(DiffLineKind::Added), "+");
        assert_eq!(line_marker(DiffLineKind::Removed), "-");
        assert_eq!(line_marker(DiffLineKind::Context), " ");
    }

    #[test]
    fn lineno_cell_formats_present_and_absent_numbers() {
        assert_eq!(lineno_cell(Some(42)), "42");
        assert_eq!(lineno_cell(None), "");
    }
}

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
//! The diff body uses "SF Mono" — macOS's own system monospace face (also
//! what Terminal.app, Xcode, and Console set their monospace text in), with
//! "Menlo" and "Monaco" — macOS's two long-standing built-in monospace
//! fonts — as fallbacks, then "Courier New" as a last resort present on
//! effectively every platform. `gpui`'s font resolution falls through this
//! list if an earlier name doesn't resolve in the current font context,
//! rather than silently substituting the UI's proportional face for code.
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
use crate::theme::Theme;
use crate::ui;

const MONOSPACE_FONT: &str = "SF Mono";
const MONOSPACE_FALLBACKS: &[&str] = &["Menlo", "Monaco", "Courier New"];

/// Estimated advance width, in pixels, of one monospace character at the
/// diff body's 12px text size — used only to size the line-number gutter
/// (see [`gutter_width`]); gpui has no API to measure real shaped text
/// outside of an actual layout pass, so this is a deliberate approximation
/// (roughly 0.6em, typical for a monospace face) rather than an exact value.
const GUTTER_CHAR_WIDTH: f32 = 7.2;
/// Horizontal padding inside a gutter column, both sides combined.
const GUTTER_PADDING: f32 = 10.0;

fn diff_font() -> Font {
    let mut f = font(MONOSPACE_FONT);
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

/// Low-alpha background tint for a line's kind, so the color reads as "this
/// line changed" without turning into a saturated block that fights with
/// the text sitting on top of it. `None` for context lines — they get no
/// tint at all, not even a neutral one, so the eye lands on what changed.
fn line_tint(kind: DiffLineKind, theme: &Theme) -> Option<Hsla> {
    match kind {
        DiffLineKind::Added => Some(Hsla {
            a: 0.14,
            ..theme.success
        }),
        DiffLineKind::Removed => Some(Hsla {
            a: 0.14,
            ..theme.danger
        }),
        DiffLineKind::Context => None,
    }
}

fn marker_color(kind: DiffLineKind, theme: &Theme) -> Hsla {
    match kind {
        DiffLineKind::Added => theme.success,
        DiffLineKind::Removed => theme.danger,
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
        .gap(px(8.0))
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
    div()
        .flex()
        .flex_col()
        .gap(px(22.0))
        .w_full()
        .children(diffs.iter().map(|diff| render_diff(diff, theme)))
        .into_any_element()
}

fn render_header(diff: &FileDiff, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(12.5))
                .text_color(theme.text)
                .child(diff.path.clone()),
        )
        .child(ui::pill(
            status_label(diff.status),
            status_color(diff.status, theme),
        ))
}

fn truncated_banner(theme: &Theme) -> impl IntoElement {
    div()
        .w_full()
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .bg(Hsla {
            a: 0.14,
            ..theme.warning
        })
        .text_size(px(11.0))
        .text_color(theme.warning)
        .child("Diff truncated at 2,000 lines for this file — later changes in it aren't shown.")
}

fn empty_note(text: &'static str, theme: &Theme) -> AnyElement {
    div()
        .w_full()
        .py(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.5))
        .text_color(theme.text_tertiary)
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
        .flex()
        .flex_col()
        .w_full()
        .rounded(px(6.0))
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
            div()
                .w_full()
                .px(px(10.0))
                .py(px(4.0))
                .bg(theme.item_wash)
                .whitespace_nowrap()
                .text_size(px(11.0))
                .text_color(theme.text_tertiary)
                .child(hunk.header.clone()),
        )
        .children(
            hunk.lines
                .iter()
                .map(|line| render_line(line, gutter_px, theme)),
        )
}

fn render_line(line: &DiffLine, gutter_px: f32, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .w_full()
        .when_some(line_tint(line.kind, theme), |d, c| d.bg(c))
        .child(lineno_col(line.old_lineno, gutter_px, theme))
        .child(lineno_col(line.new_lineno, gutter_px, theme))
        .child(
            div()
                .flex_none()
                .w(px(14.0))
                .text_size(px(12.0))
                .text_color(marker_color(line.kind, theme))
                .child(line_marker(line.kind)),
        )
        .child(
            // Deliberately no `min_w_0()`/`truncate()` here — see the module
            // doc's "Long lines" section for why that's what lets this row
            // overflow into the ancestor's horizontal scroll instead of
            // wrapping or clipping.
            div()
                .flex_1()
                .whitespace_nowrap()
                .pr(px(16.0))
                .text_size(px(12.0))
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
        .px(px(4.0))
        .text_size(px(11.0))
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

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
use gpui::{div, px, AnyElement, Hsla, SharedString};

use crate::data::{DiffHunk, DiffLine, DiffLineKind, FileDiff};
use crate::file_browser::{status_color, status_label};
use crate::theme::{Theme, RADIUS_CONTROL, SPACE_16, SPACE_24, SPACE_4, SPACE_8};
use crate::ui;

/// Horizontal padding inside a gutter column, both sides combined.
const GUTTER_PADDING: f32 = 10.0;

/// Opts an `.overflow_x_scroll()` element out of gpui 0.2.2's default "either
/// axis" wheel routing.
///
/// `elements/div.rs`'s scroll-wheel handler never calls `stop_propagation()`,
/// so a wheel event always keeps bubbling up to the Changes tab's own
/// `.overflow_y_scroll()` in `app/chrome.rs` regardless of this element —
/// that part already works. The actual bug: without this, the *same* pure
/// vertical delta that reaches this element also gets reinterpreted as
/// horizontal input here, because its `overflow.x == Scroll` while its
/// `overflow.y != Scroll`. So every wheel tick that lands on a diff body
/// (see the module doc's "Long lines" section — these fill most of the
/// panel) also yanks that diff sideways, and with a whole tab of stacked
/// diff bodies, that turns an intended vertical scroll into a distracting
/// horizontal jitter that reads as "scrolling doesn't work" even though the
/// ancestor's offset is quietly moving too. `restrict_scroll_to_axis` stops
/// this element from repurposing vertical input as horizontal, so a
/// vertical wheel gesture over a diff body only ever moves the ancestor.
///
/// `restrict_scroll_to_axis` is gpui's own documented opt-in fix
/// (`gpui::Style::restrict_scroll_to_axis`), but there is no `Styled`
/// builder method for it — every other builder (`.overflow_x_scroll()`,
/// `.debug_below()`, …) is just sugar over poking the field on
/// `StyleRefinement` through `Styled::style()`, so this does the same thing
/// by hand. A genuine horizontal gesture (non-zero `delta.x`, which is what
/// a real trackpad swipe or Shift+wheel produces) is untouched — this only
/// changes what happens when `delta.x` is zero.
fn restrict_scroll_to_horizontal_axis<E: Styled>(mut element: E) -> E {
    element.style().restrict_scroll_to_axis = Some(true);
    element
}

/// Number of characters needed to print the largest line number appearing
/// anywhere in `diff`'s hunks — so a 9-line file's gutter isn't as wide as a
/// 12,000-line file's, and a 12,000-line file's numbers don't get clipped by
/// a gutter sized for 3 digits. `1` for a diff with no line numbers at all
/// (e.g. no hunks), so [`gutter_width`] never computes a zero-width column.
fn gutter_digits(diff: &FileDiff) -> usize {
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
/// [`ui::CHAR_WIDTH_APPROX`]'s doc for why this is an estimate, not a
/// measurement.
fn gutter_width(digits: usize) -> f32 {
    GUTTER_PADDING + digits as f32 * ui::CHAR_WIDTH_APPROX
}

/// The `+`/`-`/` ` origin marker `git diff` prints before each line. Kept in
/// its own fixed column in the gutter rather than prefixed onto the line's
/// own text, so a line that itself starts with `+` or `-` (a shell script, a
/// diff-of-a-diff) can never be confused with the marker.
fn line_marker(kind: DiffLineKind) -> &'static str {
    match kind {
        DiffLineKind::Added => "+",
        DiffLineKind::Removed => "-",
        DiffLineKind::Context => " ",
    }
}

/// Text for one gutter cell: the line number, or blank when this side of the
/// diff has none (an added line has no old number; a removed line has no
/// new one).
fn lineno_cell(n: Option<u32>) -> String {
    n.map(|n| n.to_string()).unwrap_or_default()
}

/// Background wash for a line's kind — `theme.diff_add_wash`/
/// `diff_del_wash`, not an ad-hoc tint: washing the *background* and
/// tinting only the `+`/`-` marker ([`marker_color`]) is what keeps a long
/// diff's body text readable, rather than coloring the whole line's text.
/// `None` for context lines — they get no tint at all, not even a neutral
/// one, so the eye lands on what changed.
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
        // keeps the same 0.10 alpha those use, for consistency.
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
    restrict_scroll_to_horizontal_axis(
        div()
            .id(id)
            // The diff body is a self-contained bordered block at
            // `RADIUS_CONTROL`.
            .rounded(px(RADIUS_CONTROL))
            .flex()
            .flex_col()
            .w_full()
            .border_1()
            .border_color(theme.border)
            .overflow_x_scroll()
            .font(ui::mono_font()),
    )
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
            // Hunk headers get `diff_hunk_bg`, a dedicated token, not a
            // generic hover wash.
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
/// glyph at the diff body's `TEXT_SM` text size; named because it doesn't
/// fit the `SPACE_*` scale.
const MARKER_COLUMN_WIDTH: f32 = 14.0;

fn render_line(line: &DiffLine, gutter_px: f32, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .w_full()
        // Wash the line's *background*; the marker color below is the only
        // thing that carries the added/removed tint into the text itself —
        // coloring the whole line's text would make a long diff unreadable.
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
                .child(ui::non_empty_or_space(&line.text).to_string()),
        )
}

fn lineno_col(n: Option<u32>, width: f32, theme: &Theme) -> impl IntoElement {
    div()
        .flex_none()
        .w(px(width))
        .px(px(SPACE_4))
        // Right-aligned so a run of different-width line numbers still
        // lines up on their trailing digit, rather than only sharing a
        // left edge.
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
    fn gutter_width_matches_padding_plus_digits_times_char_width() {
        assert_eq!(
            gutter_width(3),
            GUTTER_PADDING + 3.0 * ui::CHAR_WIDTH_APPROX
        );
    }

    // ---------------- restrict_scroll_to_horizontal_axis ----------------
    //
    // Headless regression coverage for the scroll-hijack bug (see the
    // function's own doc comment): a `TestAppContext`-driven `gpui::test`
    // exercises the real `elements/div.rs` wheel-dispatch code (hit-testing
    // included — `add_empty_window`/`draw` run a real prepaint+paint pass,
    // so `Window::mouse_hit_test` is genuinely populated, not stubbed), so
    // this is exactly the "reachable headlessly" case, not an event-routing
    // behavior `TestAppContext` cannot drive.

    use gpui::{
        point, size, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent, TestAppContext, Window,
    };

    /// Mirrors the real nesting this bug lives in: an outer vertically-
    /// scrolling container (`app/chrome.rs`'s `changes-scroll`/
    /// `file-diff-scroll`) wrapping an inner horizontally-scrolling one
    /// (`render_hunks`'s diff body). Sized so both axes genuinely overflow
    /// — asserted below — rather than trusting the layout by construction.
    struct ScrollNestingTestView {
        outer: ScrollHandle,
        inner: ScrollHandle,
    }

    impl Render for ScrollNestingTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("outer")
                .w(px(200.))
                .h(px(150.))
                .overflow_y_scroll()
                .track_scroll(&self.outer)
                .child(restrict_scroll_to_horizontal_axis(
                    div()
                        .id("inner")
                        .w(px(150.))
                        .h(px(400.))
                        .overflow_x_scroll()
                        .track_scroll(&self.inner)
                        .child(div().w(px(2000.)).h(px(400.))),
                ))
        }
    }

    #[gpui::test]
    fn vertical_wheel_over_a_horizontally_scrolling_child_moves_only_the_ancestor(
        cx: &mut TestAppContext,
    ) {
        let cx = cx.add_empty_window();
        let outer = ScrollHandle::new();
        let inner = ScrollHandle::new();

        cx.draw(point(px(0.), px(0.)), size(px(200.), px(150.)), |_, cx| {
            cx.new(|_| ScrollNestingTestView {
                outer: outer.clone(),
                inner: inner.clone(),
            })
        });

        // Sanity-check the fixture actually overflows on both axes — a
        // false pass from a fixture that never needed to scroll would prove
        // nothing.
        assert!(
            outer.max_offset().height > px(0.),
            "fixture bug: outer has nothing to scroll vertically"
        );
        assert!(
            inner.max_offset().width > px(0.),
            "fixture bug: inner has nothing to scroll horizontally"
        );

        // A pure vertical wheel gesture (delta.x == 0) landing on the inner
        // (horizontally-scrolling) element — same as a diff body filling
        // the panel under the user's cursor.
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(50.), px(50.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
            ..Default::default()
        });

        assert_eq!(
            inner.offset().x,
            px(0.),
            "a pure vertical wheel gesture must not be reinterpreted as this \
             element's own horizontal scroll"
        );
        assert_eq!(
            outer.offset().y,
            px(-40.),
            "the same vertical wheel gesture must still reach the ancestor's \
             vertical scroll (gpui's wheel handler never calls \
             stop_propagation, so this was never the part that was broken)"
        );
    }
}

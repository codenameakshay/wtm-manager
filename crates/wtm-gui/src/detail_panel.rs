//! The detail panel: everything known about the selected worktree, rendered
//! as a fixed-width inspector down the right edge of the window.
//!
//! Pure rendering only, in the spirit of [`crate::worktree_list`]: this
//! module takes already-loaded [`WorktreeInfo`] and [`WorktreeDetails`]
//! values and turns them into elements. It must never call git or spawn a
//! task — that stays in [`crate::data`] and the view that owns the panel's
//! state, which is what lets `details` legitimately be `None` (still
//! loading) without this module knowing anything about *why*.
//!
//! `WorktreeDetails::commits` (see `wtm::worktree::CommitLine`) only carries
//! an abbreviated commit id and its first summary line — no author or
//! timestamp — so the commit rows below show sha + subject only.
//! [`relative_time`] is still provided as a tested, ready-to-use formatter
//! for the day `CommitLine` grows a timestamp field.

use gpui::prelude::*;
use gpui::{div, px, AnyElement, App, SharedString};
use wtm::model::WorktreeInfo;
use wtm::worktree::{CommitLine, WorktreeDetails};

use crate::assets::icons;
use crate::theme::Theme;
use crate::ui;

/// Fixed width of the panel, so the parent can size the layout around it
/// without measuring content.
pub const WIDTH: f32 = 320.0;

/// Fixed width of a fact row's label column (`fact_row`, `skeleton_fact_row`).
/// Wide enough to fit "Ahead/Behind" — the longest label in use — on one
/// line at its 11.5px text size; see `fact_row`'s doc for why this also
/// needs `.truncate()` as a backstop rather than relying on width alone.
const LABEL_WIDTH: f32 = 88.0;

/// Render the detail panel for `info`, with `details` loaded asynchronously
/// by the caller (`None` while still loading).
pub fn render(
    info: &WorktreeInfo,
    details: Option<&WorktreeDetails>,
    cx: &App,
) -> impl IntoElement {
    let theme = Theme::of(cx);

    div()
        .w(px(WIDTH))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .bg(theme.raised)
        .border_l_1()
        .border_color(theme.border)
        .child(render_header(info, &theme))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .flex()
                .flex_col()
                .gap(px(18.0))
                .px(px(16.0))
                .py(px(14.0))
                .child(render_facts(info, details, &theme))
                .child(render_status(info, &theme))
                .child(render_commits(details, &theme)),
        )
}

/// Branch name, the `main` badge, and a lock indicator — the same badges
/// `worktree_list` shows on a row, so recognizing the selected worktree in
/// the panel takes no relearning.
fn render_header(info: &WorktreeInfo, theme: &Theme) -> impl IntoElement {
    div()
        .w_full()
        .min_w_0()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(16.0))
        .py(px(14.0))
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(14.0))
                .text_color(theme.text)
                .child(info.display_name().to_string()),
        )
        .when(info.is_main, |this| {
            this.child(
                div()
                    .flex_none()
                    .px(px(5.0))
                    .rounded(px(4.0))
                    .text_size(px(10.5))
                    .bg(theme.item_wash)
                    .text_color(theme.text_tertiary)
                    .child("main"),
            )
        })
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
    div()
        .w_full()
        .min_w_0()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(7.0))
        .child(fact_row(
            "Path",
            truncate_path_tail(&info.path.display().to_string(), 40),
            theme,
        ))
        .child(fact_row(
            "HEAD",
            info.head.clone().unwrap_or_else(|| "-".to_string()),
            theme,
        ))
        .child(render_upstream_row(details, theme))
        .child(render_remote_row(details, theme))
        .child(render_ahead_behind_row(info, theme))
}

fn render_upstream_row(details: Option<&WorktreeDetails>, theme: &Theme) -> AnyElement {
    match details {
        None => skeleton_fact_row("Upstream", theme).into_any_element(),
        Some(details) => fact_row(
            "Upstream",
            details.upstream.clone().unwrap_or_else(|| "-".to_string()),
            theme,
        )
        .into_any_element(),
    }
}

fn render_remote_row(details: Option<&WorktreeDetails>, theme: &Theme) -> AnyElement {
    match details {
        None => skeleton_fact_row("Remote", theme).into_any_element(),
        Some(details) => fact_row(
            "Remote",
            remote_name(details.upstream.as_deref()).unwrap_or_else(|| "-".to_string()),
            theme,
        )
        .into_any_element(),
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
    fact_row("Ahead/Behind", value, theme)
}

/// One label/value line: an 11.5px `text_tertiary` label, a 12px value.
fn fact_row(
    label: &'static str,
    value: impl Into<SharedString>,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_baseline()
        .gap(px(10.0))
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
                .text_size(px(11.5))
                .text_color(theme.text_tertiary)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(12.0))
                .text_color(theme.text_secondary)
                .child(value.into()),
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
        .gap(px(10.0))
        .child(
            div()
                .flex_none()
                .w(px(LABEL_WIDTH))
                .truncate()
                .text_size(px(11.5))
                .text_color(theme.text_tertiary)
                .child(label),
        )
        .child(
            div()
                .w(px(88.0))
                .h(px(10.0))
                .rounded(px(3.0))
                .bg(theme.item_wash),
        )
}

/// Status pills, in the same order and with the same vocabulary as
/// `worktree_list`'s row pills, so the list and the panel never disagree
/// about what a badge means.
fn render_status(info: &WorktreeInfo, theme: &Theme) -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .flex_wrap()
        .gap(px(10.0))
        .children(status_pills(info, theme))
}

fn status_pills(info: &WorktreeInfo, theme: &Theme) -> Vec<AnyElement> {
    if info.is_missing {
        return vec![ui::pill("missing", theme.danger).into_any_element()];
    }

    let Some(status) = &info.status else {
        return vec![div()
            .text_size(px(11.5))
            .text_color(theme.text_ghost)
            .child("status unknown")
            .into_any_element()];
    };

    let mut pills = Vec::new();
    if status.dirty {
        pills.push(ui::pill("dirty", theme.warning).into_any_element());
    }
    if let Some(ahead) = status.ahead.filter(|n| *n > 0) {
        pills.push(ui::pill(format!("{ahead} ahead"), theme.success).into_any_element());
    }
    if let Some(behind) = status.behind.filter(|n| *n > 0) {
        pills.push(ui::pill(format!("{behind} behind"), theme.info).into_any_element());
    }
    if status.upstream_gone {
        pills.push(ui::pill("gone", theme.danger).into_any_element());
    }
    if status.merged {
        pills.push(ui::pill("merged", theme.text_tertiary).into_any_element());
    }
    pills
}

/// "Recent commits": a skeleton while `details` loads, an honest empty state
/// when the worktree genuinely has none, otherwise compact sha/subject rows.
fn render_commits(details: Option<&WorktreeDetails>, theme: &Theme) -> impl IntoElement {
    div()
        .w_full()
        .min_w_0()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex_none()
                .text_size(px(11.5))
                .text_color(theme.text_tertiary)
                .child("Recent commits"),
        )
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
        .gap(px(9.0))
        .children((0..4).map(|_| {
            div()
                .h(px(11.0))
                .w_full()
                .rounded(px(3.0))
                .bg(theme.item_wash)
        }))
}

fn render_no_commits(theme: &Theme) -> impl IntoElement {
    div()
        .text_size(px(12.0))
        .text_color(theme.text_ghost)
        .child("No commits yet")
}

fn render_commit_list(commits: &[CommitLine], theme: &Theme) -> impl IntoElement {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .children(
            commits
                .iter()
                .map(|commit| render_commit_row(commit, theme)),
        )
}

/// One compact commit row: short sha in `text_ghost`, subject truncated to a
/// single line. `CommitLine` carries no author or timestamp (see the module
/// doc comment), so those columns are intentionally absent rather than
/// faked.
fn render_commit_row(commit: &CommitLine, theme: &Theme) -> impl IntoElement {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .flex_none()
                .text_size(px(11.0))
                .text_color(theme.text_ghost)
                .child(commit.id.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(12.0))
                .text_color(theme.text_secondary)
                .child(if commit.summary.is_empty() {
                    "(no message)".to_string()
                } else {
                    commit.summary.clone()
                }),
        )
}

/// The remote name is the segment before the first `/` in the upstream
/// shorthand (`"origin/main"` -> `"origin"`). `WorktreeDetails` has no
/// separate remote field, so this is derived rather than duplicated data.
fn remote_name(upstream: Option<&str>) -> Option<String> {
    upstream.and_then(|u| u.split_once('/').map(|(remote, _)| remote.to_string()))
}

/// Condense a long path to fit the panel width, keeping the readable tail —
/// the worktree's own directory name matters far more than a prefix shared
/// by every worktree of the repo.
fn truncate_path_tail(path: &str, max_chars: usize) -> String {
    let len = path.chars().count();
    if len <= max_chars {
        return path.to_string();
    }
    let tail_len = max_chars.saturating_sub(1); // room for the leading ellipsis
    let tail: String = path.chars().skip(len - tail_len).collect();
    format!("…{tail}")
}

/// Format a Unix timestamp as a short relative-time label ("just now", "5m",
/// "3h", "2d", "3w", "5mo", "2y"), given the current time as a Unix
/// timestamp. `now` is a parameter rather than read from the clock so this
/// stays pure and testable. `unix_secs` in the future relative to `now`
/// (clock skew, or simply a bad timestamp) clamps to "just now" instead of
/// computing — and printing — a negative duration.
//
// No call site yet: `CommitLine` (see the module doc above) carries no
// timestamp for this to format. Kept — allowed, not deleted — because it is
// already written, tested, and exactly what a commit row needs the day
// `CommitLine` grows one; deleting a correct, documented formatter only to
// retype it later would be pure churn.
#[allow(dead_code)]
pub fn relative_time(unix_secs: i64, now: i64) -> String {
    let delta = now.saturating_sub(unix_secs);
    if delta < 60 {
        return "just now".to_string();
    }

    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;

    if delta < HOUR {
        format!("{}m", delta / MINUTE)
    } else if delta < DAY {
        format!("{}h", delta / HOUR)
    } else if delta < WEEK {
        format!("{}d", delta / DAY)
    } else if delta < MONTH {
        format!("{}w", delta / WEEK)
    } else if delta < YEAR {
        format!("{}mo", delta / MONTH)
    } else {
        format!("{}y", delta / YEAR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000;

    #[test]
    fn zero_delta_is_just_now() {
        assert_eq!(relative_time(NOW, NOW), "just now");
    }

    #[test]
    fn future_timestamp_clamps_to_just_now() {
        // Clock skew: the commit's timestamp is ahead of `now`. Must not
        // panic or print a negative duration.
        assert_eq!(relative_time(NOW + 3600, NOW), "just now");
        assert_eq!(relative_time(i64::MAX, NOW), "just now");
    }

    #[test]
    fn seconds_are_just_now() {
        assert_eq!(relative_time(NOW - 1, NOW), "just now");
        assert_eq!(relative_time(NOW - 59, NOW), "just now");
    }

    #[test]
    fn minute_boundary() {
        assert_eq!(relative_time(NOW - 60, NOW), "1m");
        assert_eq!(relative_time(NOW - 3599, NOW), "59m");
    }

    #[test]
    fn hour_boundary() {
        assert_eq!(relative_time(NOW - 3600, NOW), "1h");
        assert_eq!(relative_time(NOW - 86399, NOW), "23h");
    }

    #[test]
    fn day_boundary() {
        assert_eq!(relative_time(NOW - 86400, NOW), "1d");
        assert_eq!(relative_time(NOW - (7 * 86400 - 1), NOW), "6d");
    }

    #[test]
    fn week_boundary() {
        assert_eq!(relative_time(NOW - 7 * 86400, NOW), "1w");
        assert_eq!(relative_time(NOW - (30 * 86400 - 1), NOW), "4w");
    }

    #[test]
    fn month_boundary() {
        assert_eq!(relative_time(NOW - 30 * 86400, NOW), "1mo");
        assert_eq!(relative_time(NOW - (365 * 86400 - 1), NOW), "12mo");
    }

    #[test]
    fn year_boundary() {
        assert_eq!(relative_time(NOW - 365 * 86400, NOW), "1y");
        assert_eq!(relative_time(NOW - 2 * 365 * 86400, NOW), "2y");
    }

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

    #[test]
    fn remote_name_splits_on_first_slash() {
        assert_eq!(remote_name(Some("origin/main")), Some("origin".to_string()));
        assert_eq!(
            remote_name(Some("origin/feature/x")),
            Some("origin".to_string())
        );
        assert_eq!(remote_name(None), None);
    }
}

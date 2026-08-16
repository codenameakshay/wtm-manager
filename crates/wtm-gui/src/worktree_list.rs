//! The worktree list: the app's main surface.
//!
//! Each worktree is a two-line card — branch on top with its status pills,
//! path and HEAD beneath in muted meta text — rather than a spreadsheet row.
//! A worktree has one identity (its branch) and a few facts about it, and the
//! card says so; columns would spend most of their width on padding and make
//! the branch, the thing you actually scan for, no more prominent than a SHA.
//!
//! Nothing here touches git: rows are [`WorktreeInfo`] values loaded by
//! [`crate::data`].

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::prelude::*;
use gpui::{div, px, App, Div, Stateful};
use wtm::model::WorktreeInfo;

use crate::assets::icons;
use crate::theme::Theme;
use crate::ui;

/// How the worktree list orders its rows, selectable via the list
/// toolbar's sort control (`app::chrome::render_sort_control`). Kept only
/// for the current session — `WtmApp::sort_mode`'s own doc explains why it
/// isn't persisted to `prefs.rs` yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortMode {
    /// Main worktree first, then every other row alphabetically by branch
    /// (case-insensitive) — the list's original ordering.
    #[default]
    Name,
    /// Main worktree first, then most-recently-committed-to first.
    Recent,
    /// Main worktree first, then whichever rows most need attention:
    /// dirty, then ahead/behind an upstream, then clean.
    Status,
}

impl SortMode {
    /// Every mode, in the order the toolbar's segmented control shows them.
    pub const ALL: [SortMode; 3] = [SortMode::Name, SortMode::Recent, SortMode::Status];
}

/// Label for `mode` in the toolbar's sort control.
pub fn sort_mode_label(mode: SortMode) -> &'static str {
    match mode {
        SortMode::Name => "Name",
        SortMode::Recent => "Recent",
        SortMode::Status => "Status",
    }
}

/// Sort `rows` per `mode`, in place.
///
/// The main worktree is always pinned first, in every mode: it is the
/// repository's anchor — what nearly every other worktree branches from,
/// and the one row every repo-scoped action (Prune, the config file
/// Settings can reveal) implicitly concerns — not just another row that
/// happens to alphabetize first or was committed to most recently. Burying
/// it under a feature branch touched five minutes ago would make the one
/// row users most reliably orient around the *least* discoverable one, in
/// exactly the mode (`Recent`) where that would happen most often.
///
/// `activity` (HEAD commit unix-time by worktree path, from
/// `data::worktree_activity`) drives `Recent`'s ordering only; `Status`
/// reads a row's own `status` field, `Name` neither. Any of those can be
/// incomplete (activity still loading, status not yet computed) — a row
/// missing the active mode's key sorts after every row that has one,
/// never into some arbitrary position, so a partially-loaded list reads as
/// "the unknowns are at the bottom" rather than looking scrambled.
pub fn sort_rows(rows: &mut [WorktreeInfo], mode: SortMode, activity: &HashMap<PathBuf, i64>) {
    rows.sort_by(|a, b| {
        // Main pinned first, in every mode — see this function's doc. At
        // most one row is ever `is_main`, so this ordering is always
        // well-defined (never two rows both claiming to sort first).
        match (a.is_main, b.is_main) {
            (true, false) => return std::cmp::Ordering::Less,
            (false, true) => return std::cmp::Ordering::Greater,
            _ => {}
        }
        match mode {
            SortMode::Name => name_key(a).cmp(&name_key(b)),
            SortMode::Recent => recent_key(a, activity)
                .cmp(&recent_key(b, activity))
                .then_with(|| name_key(a).cmp(&name_key(b))),
            SortMode::Status => status_key(a)
                .cmp(&status_key(b))
                .then_with(|| name_key(a).cmp(&name_key(b))),
        }
    });
}

/// Case-insensitive branch/display name — `Name`'s own primary key, and
/// the tie-break every other mode falls back to so two rows with an
/// otherwise-equal key still land in a stable, predictable order.
fn name_key(info: &WorktreeInfo) -> String {
    info.display_name().to_lowercase()
}

/// `Recent`'s sort key: a worktree with known activity always sorts before
/// one without (the `bool` component), and within "known" a later
/// (more recent) timestamp sorts first — `Reverse` turns the ordinary
/// ascending comparison `sort_by` performs into "largest first" without a
/// second, separately-reasoned comparator.
fn recent_key(
    info: &WorktreeInfo,
    activity: &HashMap<PathBuf, i64>,
) -> (bool, std::cmp::Reverse<i64>) {
    match activity.get(&info.path) {
        Some(&t) => (false, std::cmp::Reverse(t)),
        None => (true, std::cmp::Reverse(i64::MIN)),
    }
}

/// `Status`'s sort key: needs-attention rows first. `dirty` outranks
/// ahead/behind (uncommitted work is more at risk of being lost than a
/// commit that simply hasn't been pushed/pulled yet), which outranks a
/// clean-or-unknown row. Unknown status (not yet computed) is folded into
/// the same bucket as clean rather than treated as urgent — claiming a row
/// needs attention before its status has even been computed would be a
/// guess, not a fact.
fn status_key(info: &WorktreeInfo) -> u8 {
    match &info.status {
        Some(status) if status.dirty => 0,
        Some(status)
            if status.ahead.is_some_and(|n| n > 0) || status.behind.is_some_and(|n| n > 0) =>
        {
            1
        }
        _ => 2,
    }
}

/// One worktree card. Returns a stateful element so the caller can attach
/// click handling without this module knowing about the app's state.
///
/// `age`, when known, is `data::relative_age` of the worktree's HEAD
/// commit — shown muted at the far right of the meta line, right of the
/// existing path/status/HEAD info. `None` (unknown activity: still
/// loading, or no resolvable HEAD) renders nothing rather than a
/// placeholder — an empty space reads better than a guess.
pub fn render_row(
    info: &WorktreeInfo,
    row_ix: usize,
    selected: bool,
    awaiting_status: bool,
    age: Option<String>,
    cx: &App,
) -> Stateful<Div> {
    let theme = Theme::of(cx);

    ui::row(("worktree", row_ix), selected, &theme)
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .overflow_hidden()
                .line_height(px(18.0))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.0))
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
                }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .text_size(px(11.5))
                .line_height(px(15.0))
                .child(div().flex().flex_1().min_w_0().child(ui::meta(
                    icons::FOLDER,
                    display_path(info),
                    &theme,
                )))
                .children(status_pills(info, awaiting_status, &theme))
                .when_some(info.head.clone(), |this, head| {
                    this.child(div().flex_none().text_color(theme.text_ghost).child(head))
                })
                .when_some(age, |this, age| {
                    this.child(div().flex_none().text_color(theme.text_ghost).child(age))
                }),
        )
}

/// The count text at the head of the list: "N worktrees" normally, or "N of
/// M worktrees" while a filter (`shown < total`) narrows what is visible —
/// the exact wording the type-to-filter feature promises, so the header
/// itself is proof the filter is doing something rather than the list
/// simply being short.
pub fn render_header(shown: usize, total: usize, loading: bool, cx: &App) -> impl IntoElement {
    let theme = Theme::of(cx);

    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_size(px(12.5))
        .text_color(theme.text_tertiary)
        .child(if shown == total {
            match total {
                1 => "1 worktree".to_string(),
                n => format!("{n} worktrees"),
            }
        } else {
            format!("{shown} of {total} worktrees")
        })
        .when(loading, |this| {
            this.child(
                div()
                    .text_color(theme.text_ghost)
                    .child("· loading status…"),
            )
        })
}

/// Shown in place of the list when a repository has no worktrees.
pub fn render_empty(cx: &App) -> impl IntoElement {
    empty_state(
        "No worktrees yet",
        "Create one from a branch to get started.",
        cx,
    )
}

/// Shown when no repository is selected at all.
pub fn render_no_repo(cx: &App) -> impl IntoElement {
    empty_state(
        "No repository open",
        "Run `wtm` inside a git repository to add it here.",
        cx,
    )
}

fn empty_state(title: &'static str, hint: &'static str, cx: &App) -> impl IntoElement {
    let theme = Theme::of(cx);

    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .items_center()
        .justify_center()
        .child(
            div()
                .text_size(px(13.0))
                .text_color(theme.text_secondary)
                .child(title),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(theme.text_ghost)
                .child(hint),
        )
}

/// Status pills for a row, in the order they matter when scanning: what
/// blocks you (dirty, missing), then how far the branch has drifted.
///
/// Missing status is shown as a placeholder rather than as "clean" — calling a
/// dirty worktree clean is the one wrong answer here, so an unknown state
/// always looks unknown.
fn status_pills(
    info: &WorktreeInfo,
    awaiting_status: bool,
    theme: &Theme,
) -> Vec<gpui::AnyElement> {
    if info.is_missing {
        return vec![ui::pill("missing", theme.danger).into_any_element()];
    }

    let Some(status) = &info.status else {
        let text = if awaiting_status { "…" } else { "-" };
        return vec![div()
            .flex_none()
            .text_color(theme.text_ghost)
            .child(text)
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

/// Home-relative path, so the common case reads as `~/code/project` rather
/// than an absolute path that pushes the interesting part off screen.
fn display_path(info: &WorktreeInfo) -> String {
    let path = info.path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path,
    }
}

#[cfg(test)]
mod sort_tests {
    use wtm::model::WorktreeStatus;

    use super::*;

    /// A worktree with a given name/main-ness and, optionally, a status.
    /// `path` is always `/tmp/<name>` — unique per name, which is all
    /// `sort_rows`'s `activity` lookup (keyed by path) needs.
    fn wt(name: &str, is_main: bool, status: Option<WorktreeStatus>) -> WorktreeInfo {
        WorktreeInfo {
            name: name.to_string(),
            path: PathBuf::from(format!("/tmp/{name}")),
            branch: Some(name.to_string()),
            head: None,
            is_main,
            is_missing: false,
            is_locked: false,
            is_prunable: false,
            status,
        }
    }

    fn clean() -> WorktreeStatus {
        WorktreeStatus {
            dirty: false,
            ahead: None,
            behind: None,
            upstream_gone: false,
            merged: false,
        }
    }

    fn dirty() -> WorktreeStatus {
        WorktreeStatus {
            dirty: true,
            ..clean()
        }
    }

    fn behind(n: usize) -> WorktreeStatus {
        WorktreeStatus {
            behind: Some(n),
            ..clean()
        }
    }

    fn names(rows: &[WorktreeInfo]) -> Vec<&str> {
        rows.iter().map(|r| r.display_name()).collect()
    }

    #[test]
    fn name_mode_pins_main_first_then_sorts_alphabetically_case_insensitively() {
        let mut rows = vec![
            wt("zebra", false, None),
            wt("main", true, None),
            wt("Apple", false, None),
            wt("banana", false, None),
        ];
        sort_rows(&mut rows, SortMode::Name, &HashMap::new());
        assert_eq!(names(&rows), vec!["main", "Apple", "banana", "zebra"]);
    }

    #[test]
    fn recent_mode_pins_main_first_then_orders_by_most_recent_commit() {
        let mut rows = vec![
            wt("old", false, None),
            wt("main", true, None),
            wt("new", false, None),
            wt("mid", false, None),
        ];
        let activity: HashMap<PathBuf, i64> = HashMap::from([
            (PathBuf::from("/tmp/old"), 100),
            (PathBuf::from("/tmp/new"), 300),
            (PathBuf::from("/tmp/mid"), 200),
        ]);
        sort_rows(&mut rows, SortMode::Recent, &activity);
        assert_eq!(names(&rows), vec!["main", "new", "mid", "old"]);
    }

    #[test]
    fn recent_mode_puts_unknown_activity_after_every_known_row() {
        let mut rows = vec![
            wt("no-data", false, None),
            wt("main", true, None),
            wt("has-data", false, None),
        ];
        let activity: HashMap<PathBuf, i64> = HashMap::from([(PathBuf::from("/tmp/has-data"), 42)]);
        sort_rows(&mut rows, SortMode::Recent, &activity);
        assert_eq!(names(&rows), vec!["main", "has-data", "no-data"]);
    }

    #[test]
    fn status_mode_pins_main_first_then_dirty_then_ahead_behind_then_clean() {
        let mut rows = vec![
            wt("clean-one", false, Some(clean())),
            wt("main", true, Some(dirty())), // even a dirty main worktree stays first
            wt("stale", false, Some(behind(3))),
            wt("wip", false, Some(dirty())),
            wt("unknown", false, None),
        ];
        sort_rows(&mut rows, SortMode::Status, &HashMap::new());
        assert_eq!(
            names(&rows),
            vec!["main", "wip", "stale", "clean-one", "unknown"]
        );
    }

    #[test]
    fn status_mode_treats_unknown_status_the_same_as_clean_not_as_urgent() {
        let mut rows = vec![
            wt("main", true, None),
            wt("unknown", false, None),
            wt("dirty-one", false, Some(dirty())),
        ];
        sort_rows(&mut rows, SortMode::Status, &HashMap::new());
        // `unknown` must not jump ahead of a genuinely dirty row just
        // because its status hasn't been computed yet.
        assert_eq!(names(&rows), vec!["main", "dirty-one", "unknown"]);
    }

    #[test]
    fn every_mode_keeps_the_main_worktree_first_regardless_of_its_own_data() {
        // Main is alphabetically last, least recently active, and dirty —
        // the worst case for every other key — and must still stay first.
        let make = || {
            vec![
                wt("aardvark", false, Some(clean())),
                wt("zzz-main", true, Some(dirty())),
            ]
        };
        let activity: HashMap<PathBuf, i64> =
            HashMap::from([(PathBuf::from("/tmp/aardvark"), 1_000_000)]);

        for mode in SortMode::ALL {
            let mut rows = make();
            sort_rows(&mut rows, mode, &activity);
            assert_eq!(
                rows[0].name, "zzz-main",
                "main must sort first under {mode:?}"
            );
        }
    }
}

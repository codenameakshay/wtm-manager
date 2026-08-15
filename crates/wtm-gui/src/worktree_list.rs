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

use gpui::prelude::*;
use gpui::{div, px, App, Div, Stateful};
use wtm::model::WorktreeInfo;

use crate::assets::icons;
use crate::theme::Theme;
use crate::ui;

/// One worktree card. Returns a stateful element so the caller can attach
/// click handling without this module knowing about the app's state.
pub fn render_row(
    info: &WorktreeInfo,
    row_ix: usize,
    selected: bool,
    awaiting_status: bool,
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

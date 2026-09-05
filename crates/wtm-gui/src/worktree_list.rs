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
use gpui::{div, px, AnyElement, App, Div, FontWeight, Hsla, Stateful};
use wtm::model::{WorktreeInfo, WorktreeStatus};

use crate::assets::icons;
use crate::detail_panel::{truncate_path_tail, truncate_tail};
use crate::motion;
use crate::theme::{Theme, LIST_ROW_HEIGHT, SPACE_4, SPACE_6, SPACE_8};
use crate::ui;

/// Character floor for the path once every never-shrink element on line 2
/// is accounted for (see [`line2_layout`]) — room for a leading "…" plus a
/// handful of characters of the worktree's own directory name, the part
/// that actually disambiguates one worktree from another sharing the same
/// parent. Honored whenever there's room to; [`PATH_ABSOLUTE_MIN_CHARS`] is
/// the true last resort.
const PATH_MIN_CHARS: usize = 8;

/// The path's absolute last-resort floor, reached only if the status pills
/// alone (which never shrink — see [`line2_layout`]) already consume
/// nearly all of a pathologically narrow row. Still shows a leading "…"
/// plus a couple of characters rather than disappearing outright.
const PATH_ABSOLUTE_MIN_CHARS: usize = 3;

/// Character floor for the branch name (line 1) — line 1 only ever
/// competes with the `main` badge and the lock icon, both small and fixed,
/// so in every supported window size it has far more room than this floor
/// requires; kept only so the arithmetic in [`line1_max_chars`] can't
/// produce a zero-or-negative budget in some future, narrower layout.
const BRANCH_MIN_CHARS: usize = 6;

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
    // `sort_by_cached_key` computes each row's key exactly once (rather than
    // re-lowercasing a branch name on every comparison a plain `sort_by`
    // makes), then sorts from the cached values. The leading `bool` pins the
    // main worktree first in every mode — at most one row is ever
    // `is_main`, so this ordering is always well-defined.
    rows.sort_by_cached_key(|info| {
        let name = name_key(info);
        match mode {
            SortMode::Name => (!info.is_main, 0u8, std::cmp::Reverse(0), name),
            SortMode::Recent => {
                let (unknown, ts) = recent_key(info, activity);
                (!info.is_main, unknown as u8, ts, name)
            }
            SortMode::Status => (!info.is_main, status_key(info), std::cmp::Reverse(0), name),
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

/// One status pill/placeholder on a row's meta line: `color` is `Some` for
/// a real status pill (dot + colored label, via `ui::pill`) and `None` for
/// the plain muted placeholder text shown while status is still loading or
/// genuinely unknown. The single source of truth both [`render_status_pills`]
/// (what paints) and [`status_pills_reserve_px`] (how much room this row's
/// #1 priority — pills never shrink, never clip — reserves for it) read
/// from, so rendering and width-budgeting can never disagree about which
/// pills a row shows.
pub(crate) struct PillSpec {
    pub(crate) label: String,
    pub(crate) color: Option<Hsla>,
}

/// Label for the dirty pill: the exact count, not just "dirty". `pub(crate)`
/// so `detail_panel::status_pills` shares this exact wording.
pub(crate) fn dirty_pill_label(n: usize) -> String {
    match n {
        1 => "1 dirty".to_string(),
        n => format!("{n} dirty"),
    }
}

/// Status pills for a row, in the order they matter when scanning: what
/// blocks you (dirty, missing), then how far the branch has drifted.
///
/// Missing status is shown as a placeholder rather than as "clean" — calling a
/// dirty worktree clean is the one wrong answer here, so an unknown state
/// always looks unknown.
fn pill_specs(info: &WorktreeInfo, awaiting_status: bool, theme: &Theme) -> Vec<PillSpec> {
    if info.is_missing {
        return vec![PillSpec {
            label: "missing".to_string(),
            color: Some(theme.danger),
        }];
    }

    let Some(status) = &info.status else {
        let text = if awaiting_status { "…" } else { "-" };
        return vec![PillSpec {
            label: text.to_string(),
            color: None,
        }];
    };

    status_pill_specs(status, theme)
}

/// Colored status pills for a known, present [`WorktreeStatus`] — dirty,
/// ahead/behind, gone, merged — in the order they matter when scanning.
/// `pub(crate)` so `detail_panel::status_pills` shares this exact list
/// (and thus rendering) instead of a second, driftable copy — the list row
/// and the detail panel must never disagree about which pills a status
/// produces.
pub(crate) fn status_pill_specs(status: &WorktreeStatus, theme: &Theme) -> Vec<PillSpec> {
    let mut specs = Vec::new();
    if status.dirty {
        specs.push(PillSpec {
            label: dirty_pill_label(status.dirty_count),
            color: Some(theme.warning),
        });
    }
    if let Some(ahead) = status.ahead.filter(|n| *n > 0) {
        specs.push(PillSpec {
            label: format!("{ahead} ahead"),
            color: Some(theme.success),
        });
    }
    if let Some(behind) = status.behind.filter(|n| *n > 0) {
        specs.push(PillSpec {
            label: format!("{behind} behind"),
            color: Some(theme.info),
        });
    }
    if status.upstream_gone {
        specs.push(PillSpec {
            label: "gone".to_string(),
            color: Some(theme.danger),
        });
    }
    if status.merged {
        // `success_muted` keeps `merged` recognizably in the `success`
        // family while staying quieter than the needs-attention pills
        // (`dirty`/`gone`) that can appear earlier in this same list.
        specs.push(PillSpec {
            label: "merged".to_string(),
            color: Some(theme.success_muted),
        });
    }
    specs
}

/// Renders [`pill_specs`]'s output as a single flex-row group: a real
/// `ui::pill` for each colored spec, or plain `text_ghost` text for the
/// loading/unknown placeholder. `None` when `specs` is empty.
///
/// The colored-pill group (never the placeholder) is wrapped in
/// [`motion::fade_quick`], keyed to this row, so status resolving from a
/// placeholder into a real answer gets one fade-in rather than popping in.
fn render_status_pills(
    specs: &[PillSpec],
    theme: &Theme,
    row_ix: usize,
    cx: &App,
) -> Option<AnyElement> {
    if specs.is_empty() {
        return None;
    }
    let is_real = specs.iter().any(|spec| spec.color.is_some());
    let group = div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(SPACE_8))
        .children(specs.iter().map(|spec| {
            match spec.color {
                Some(color) => ui::pill(spec.label.clone(), color).into_any_element(),
                None => div()
                    .flex_none()
                    .text_color(theme.text_ghost)
                    .child(spec.label.clone())
                    .into_any_element(),
            }
        }));
    Some(if is_real {
        motion::fade_quick(("worktree-pills", row_ix), group, cx).into_any_element()
    } else {
        group.into_any_element()
    })
}

/// Reserved width for a `ui::main_badge`-shaped chip: `SPACE_6` padding on
/// both sides plus its label text — mirrors `ui::main_badge`'s own layout
/// exactly so this reservation never drifts from what it actually paints.
fn badge_reserve_px(label: &str) -> f32 {
    SPACE_6 * 2.0 + label.chars().count() as f32 * ui::CHAR_WIDTH_APPROX
}

/// Reserved width for one `ui::pill`: its 6px dot, the `SPACE_4` gap to the
/// label, and the label text — mirrors `ui::pill`'s own layout exactly, the
/// same reasoning as [`badge_reserve_px`].
fn pill_reserve_px(label: &str) -> f32 {
    6.0 + SPACE_4 + label.chars().count() as f32 * ui::CHAR_WIDTH_APPROX
}

/// Total width [`pill_specs`]' output reserves on the line: every pill's
/// (or placeholder's) own width, plus the `SPACE_8` gaps between them —
/// this row's #1 priority (pills never shrink, never clip) is enforced
/// entirely by reserving this much room for them *before* the path gets
/// whatever's left, never by shrinking a pill itself.
fn status_pills_reserve_px(specs: &[PillSpec]) -> f32 {
    if specs.is_empty() {
        return 0.0;
    }
    let content: f32 = specs
        .iter()
        .map(|spec| match spec.color {
            Some(_) => pill_reserve_px(&spec.label),
            None => spec.label.chars().count() as f32 * ui::CHAR_WIDTH_APPROX,
        })
        .sum();
    let gaps = (specs.len() - 1) as f32 * SPACE_8;
    content + gaps
}

/// Budgets line 1 (branch name, `main` badge, lock icon) against
/// `inner_width` — the row's live-computed content width (see
/// `app::chrome::WtmApp::worktree_row_card_width`). The branch name is
/// this row's heaviest text and only ever competes with the badge/lock,
/// both small and fixed, so — unlike the path on line 2 — it rarely needs
/// to lean on [`BRANCH_MIN_CHARS`] in practice: the branch name truncates
/// only after the path has nothing left to give.
fn line1_max_chars(inner_width: f32, is_main: bool, is_locked: bool) -> usize {
    let children = 1 + usize::from(is_main) + usize::from(is_locked);
    let mut reserved = 0.0;
    if is_main {
        reserved += badge_reserve_px("main");
    }
    if is_locked {
        reserved += 11.0; // ui::icon's own fixed glyph size, no label
    }
    let gaps = (children - 1) as f32 * SPACE_8;
    let budget_px = (inner_width - reserved - gaps).max(0.0);
    ((budget_px / ui::CHAR_WIDTH_APPROX).floor() as usize).max(BRANCH_MIN_CHARS)
}

/// The outcome of budgeting line 2's never-shrink elements against
/// `inner_width`: whether the sha/age are shown at all, and how many
/// characters the path gets.
struct Line2Layout {
    path_max_chars: usize,
    show_sha: bool,
    show_age: bool,
}

/// Budgets line 2 (folder icon, path, status pills, HEAD sha, relative
/// age) against `inner_width` — the row's actual, live-computed content
/// width, not a flat character cap that can't tell a 900px window from a
/// 1400px one.
///
/// Priority order (status is the reason the row exists): the status pills
/// are reserved for in full and never shrink; the sha and then the age (the
/// least essential of the two, since it's furthest from the pills that
/// matter) are the first things dropped *entirely*, before the path would
/// otherwise fall below [`PATH_MIN_CHARS`]; the path always gets *some*
/// room, shrinking first and furthest of everything on the line.
fn line2_layout(
    inner_width: f32,
    pill_specs: &[PillSpec],
    sha: Option<&str>,
    age: Option<&str>,
) -> Line2Layout {
    // The folder icon (`ui::meta`'s own 11px glyph) plus its `SPACE_4` gap
    // to the path text — always present, mirrors `ui::meta`'s own layout.
    let icon_reserve = 11.0 + SPACE_4;
    let pills_reserve = status_pills_reserve_px(pill_specs);
    let sha_reserve = sha.map(|s| s.chars().count() as f32 * ui::CHAR_WIDTH_APPROX);
    let age_reserve = age.map(|a| a.chars().count() as f32 * ui::CHAR_WIDTH_APPROX);

    let budget = |show_sha: bool, show_age: bool| -> usize {
        let mut children = 1 + pill_specs.len(); // path + pills
        let mut extra = 0.0;
        if show_sha {
            children += 1;
            extra += sha_reserve.unwrap_or(0.0);
        }
        if show_age {
            children += 1;
            extra += age_reserve.unwrap_or(0.0);
        }
        let gaps = (children.saturating_sub(1)) as f32 * SPACE_8;
        let reserved = icon_reserve + pills_reserve + extra + gaps;
        ((inner_width - reserved) / ui::CHAR_WIDTH_APPROX)
            .floor()
            .max(0.0) as usize
    };

    let everything = budget(sha.is_some(), age.is_some());
    if everything >= PATH_MIN_CHARS || (sha.is_none() && age.is_none()) {
        return Line2Layout {
            path_max_chars: everything.max(PATH_ABSOLUTE_MIN_CHARS),
            show_sha: sha.is_some(),
            show_age: age.is_some(),
        };
    }

    // Drop age first — the least essential of the two.
    let without_age = budget(sha.is_some(), false);
    if without_age >= PATH_MIN_CHARS || sha.is_none() {
        return Line2Layout {
            path_max_chars: without_age.max(PATH_ABSOLUTE_MIN_CHARS),
            show_sha: sha.is_some(),
            show_age: false,
        };
    }

    // Drop the sha too. Pills themselves are never touched by this
    // function — they were already fully reserved for above.
    Line2Layout {
        path_max_chars: budget(false, false).max(PATH_ABSOLUTE_MIN_CHARS),
        show_sha: false,
        show_age: false,
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
///
/// `card_width` is this card's own live-computed width — see
/// `app::chrome::WtmApp::worktree_row_card_width`'s doc for the full
/// derivation. [`line1_max_chars`]/[`line2_layout`] budget from it directly
/// (rather than a flat character cap that can't tell a narrow window from a
/// wide one) in priority order: pills and the `main` badge/lock never
/// shrink or clip; the branch name and then the path give way, in that
/// order; the sha and age are dropped outright before a pill ever would be.
///
/// `cx` is only ever read for [`motion::reduced`] inside
/// [`render_status_pills`]'s fade-in wrapper, never for color.
#[allow(clippy::too_many_arguments)]
pub fn render_row(
    info: &WorktreeInfo,
    row_ix: usize,
    selected: bool,
    awaiting_status: bool,
    age: Option<String>,
    card_width: f32,
    theme: &Theme,
    cx: &App,
) -> Stateful<Div> {
    // Takes `&Theme` directly rather than resolving `Theme::of(cx)` itself:
    // `app::chrome::render_list`'s `uniform_list` processor already computes
    // `self.chrome_theme(cx)` once per visible range — the copy with
    // `Theme::tab_stops` forced to `false` while a dialog covers the list
    // (see that field's doc) — and resolving it fresh here would silently
    // discard that, letting a row underneath an open dialog register as a
    // real Tab stop.
    let theme = *theme;

    // `ui::row` applies its own `SPACE_8` padding on both edges before
    // either line's content starts.
    let inner_width = (card_width - SPACE_8 * 2.0).max(0.0);

    let name_max_chars = line1_max_chars(inner_width, info.is_main, info.is_locked);
    let display_name = truncate_tail(info.display_name(), name_max_chars);
    // The box is sized to the *shown* text's own estimated width, not to
    // the full budget — a short name (e.g. "main") gets a snug box, so the
    // badge/lock beside it sit right next to the text instead of trailing
    // a wide gap sized for a name this row doesn't actually have.
    // `CHAR_WIDTH_APPROX` is a mono-face estimate reused here for this
    // proportional branch name too, deliberately conservatively: a
    // proportional glyph usually runs a little narrower, so this slightly
    // overestimates the width, which is the safe direction to be wrong in.
    let name_box_width = display_name.chars().count() as f32 * ui::CHAR_WIDTH_APPROX;

    let specs = pill_specs(info, awaiting_status, &theme);
    let sha = info.head.as_deref();
    let layout = line2_layout(inner_width, &specs, sha, age.as_deref());
    let path_text = display_path(info, layout.path_max_chars);
    let path_box_width = path_text.chars().count() as f32 * ui::CHAR_WIDTH_APPROX;

    // A two-line card at `LIST_ROW_HEIGHT`: `ui::row` owns the radius and
    // the hover/selection wash; this function only lays out the two lines
    // inside it and centers them in the fixed height, since neither line
    // alone fills it.
    ui::row(("worktree", row_ix), selected, &theme)
        .h(px(LIST_ROW_HEIGHT))
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(SPACE_4))
        .child(
            div()
                .flex()
                .min_w_0()
                .items_center()
                .gap(px(SPACE_8))
                .overflow_hidden()
                .child(
                    // The branch name must out-weigh everything else on the
                    // row: `TEXT_BASE`/500 weight, the heaviest text this
                    // card ever shows. Truncated names still get their full
                    // text via a tooltip.
                    //
                    // A definite `.w(px(..))` (not `flex_1`/`min_w_0`) is
                    // what actually avoids gpui 0.2.2's text-measurement
                    // caching bug (`detail_panel::LABEL_WIDTH`'s doc) —
                    // `.truncate()` stays on as the backstop that doc
                    // describes, not the primary truncation mechanism; the
                    // string is already shortened to `name_max_chars`
                    // before it ever reaches gpui. `.id(..)` because
                    // `.tooltip(..)` is `StatefulInteractiveElement`-only.
                    div()
                        .id(("worktree-name", row_ix))
                        .flex_none()
                        .w(px(name_box_width))
                        .truncate()
                        .text_size(px(ui::TEXT_BASE))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(display_name)
                        .tooltip(ui::tooltip(info.display_name().to_string())),
                )
                .when(info.is_main, |this| this.child(ui::main_badge(&theme)))
                .when(info.is_locked, |this| {
                    this.child(ui::icon(icons::LOCK, 11.0, theme.text_ghost))
                }),
        )
        .child(
            div()
                .flex()
                .min_w_0()
                .items_center()
                .gap(px(SPACE_8))
                .overflow_hidden()
                .text_size(px(ui::TEXT_SM))
                .child(
                    // Built by hand rather than through `ui::meta` (whose
                    // inner text has no definite width of its own) so the
                    // path text can carry one — see this function's own doc
                    // and the branch-name comment above. Visually identical
                    // to `ui::meta`'s own recipe (11px `FOLDER` icon,
                    // `SPACE_4` gap, `FONT_MONO`/`TEXT_SM`/`text_muted`
                    // label) — truncating from the *start*, keeping the
                    // worktree's own directory name.
                    div()
                        .id(("worktree-path", row_ix))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(SPACE_4))
                        .child(ui::icon(icons::FOLDER, 11.0, theme.text_ghost))
                        .child(
                            div()
                                .flex_none()
                                .w(px(path_box_width))
                                .truncate()
                                .font_family(theme.font_mono)
                                .text_color(theme.text_muted)
                                .child(path_text),
                        )
                        .tooltip(ui::tooltip(info.path.display().to_string())),
                )
                .when_some(
                    render_status_pills(&specs, &theme, row_ix, cx),
                    |this, pills| this.child(pills),
                )
                .when(layout.show_sha, |this| {
                    // HEAD is a sha in meta position — `FONT_MONO`, same as
                    // the path beside it, so columns of shas line up.
                    this.child(
                        div()
                            .flex_none()
                            .font_family(ui::FONT_MONO)
                            .text_color(theme.text_ghost)
                            .child(sha.unwrap_or_default().to_string()),
                    )
                })
                .when(layout.show_age, |this| {
                    this.child(
                        div()
                            .flex_none()
                            .text_color(theme.text_ghost)
                            .child(age.as_deref().unwrap_or("").to_string()),
                    )
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
    let count_text = if shown == total {
        match total {
            1 => "1 worktree".to_string(),
            n => format!("{n} worktrees"),
        }
    } else {
        format!("{shown} of {total} worktrees")
    };

    // The count is `flex_none()` (always its full natural width, so a
    // narrow allocation instead forces the toolbar row's own `flex_wrap()`
    // to drop the actions group to its own line rather than clipping
    // digits); the "· loading status…" suffix is disposable — it repeats
    // what the titlebar's spinner already shows — so it absorbs whatever
    // shrink pressure is left instead.
    div()
        .flex()
        .min_w_0()
        .items_center()
        .gap(px(SPACE_8))
        .text_size(px(ui::TEXT_SM))
        .text_color(theme.text_faint)
        .child(div().flex_none().child(count_text))
        .when(loading, |this| {
            this.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.text_ghost)
                    .child("· loading status…"),
            )
        })
}

/// Shown in place of the list when a repository has no worktrees — the
/// first thing a new user of this repo sees, so it gets the full
/// `ui::empty_state` treatment (icon, headline, hint, and an action) rather
/// than two lines of grey text in a void.
///
/// `action` is built by the caller (`app::chrome::WtmApp::render_list`),
/// which owns the `Context<WtmApp>` a real "New Worktree" click handler
/// needs — this module renders `WorktreeInfo` values with no such context
/// (see the module doc) and never will. An empty state whose next action is
/// one click away is the highest-value delight this app has: a brand-new
/// user's very first screen is otherwise a dead end until they discover the
/// sidebar button or the `⌘N` shortcut on their own.
pub fn render_empty(action: AnyElement, cx: &App) -> impl IntoElement {
    let theme = Theme::of(cx);
    ui::empty_state(
        icons::GIT_BRANCH,
        "No worktrees yet",
        "Create one from a branch to get started.",
        Some(action),
        &theme,
    )
}

/// Shown when no repository is selected at all. Same action-slot wiring as
/// [`render_empty`].
pub fn render_no_repo(action: AnyElement, cx: &App) -> impl IntoElement {
    let theme = Theme::of(cx);
    ui::empty_state(
        icons::FOLDER_OPEN,
        "No repository open",
        "Run `wtm` inside a git repository to add it here.",
        Some(action),
        &theme,
    )
}

/// Home-relative path (via [`ui::display_path`]), so the common case reads
/// as `~/code/project` rather than an absolute path that pushes the
/// interesting part off screen — then capped to `max_chars` (the line 2
/// budget [`line2_layout`] computed) with a leading ellipsis so the
/// worktree's own directory name (the tail) survives, the same
/// `truncate_path_tail` mechanism `detail_panel`'s Path row uses and for
/// the same reason (see that function's doc).
fn display_path(info: &WorktreeInfo, max_chars: usize) -> String {
    truncate_path_tail(&ui::display_path(&info.path), max_chars)
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn dirty_pill() -> PillSpec {
        PillSpec {
            label: "dirty".to_string(),
            color: Some(Hsla {
                h: 0.1,
                s: 0.5,
                l: 0.5,
                a: 1.0,
            }),
        }
    }

    fn gone_pill() -> PillSpec {
        PillSpec {
            label: "gone".to_string(),
            color: Some(Hsla {
                h: 0.0,
                s: 0.5,
                l: 0.5,
                a: 1.0,
            }),
        }
    }

    fn merged_pill() -> PillSpec {
        PillSpec {
            label: "merged".to_string(),
            color: Some(Hsla {
                h: 0.4,
                s: 0.5,
                l: 0.5,
                a: 1.0,
            }),
        }
    }

    #[test]
    fn dirty_pill_label_covers_singular_plural_and_zero() {
        assert_eq!(dirty_pill_label(1), "1 dirty");
        assert_eq!(dirty_pill_label(2), "2 dirty");
        assert_eq!(dirty_pill_label(42), "42 dirty");
        // No real call site renders this, but it must stay total.
        assert_eq!(dirty_pill_label(0), "0 dirty");
    }

    #[test]
    fn status_pills_reserve_is_zero_for_an_empty_row() {
        assert_eq!(status_pills_reserve_px(&[]), 0.0);
    }

    #[test]
    fn line2_layout_shows_everything_when_there_is_plenty_of_room() {
        let specs = [dirty_pill()];
        let layout = line2_layout(500.0, &specs, Some("abc1234"), Some("5mo"));
        assert!(layout.show_sha);
        assert!(layout.show_age);
        assert!(
            layout.path_max_chars >= PATH_MIN_CHARS,
            "plenty of room should give the path more than its floor, got {}",
            layout.path_max_chars
        );
    }

    #[test]
    fn line2_layout_drops_age_before_sha_under_pressure() {
        // Age (least essential) goes first.
        let specs = [dirty_pill()];
        let layout = line2_layout(200.0, &specs, Some("abc1234"), Some("5mo"));
        assert!(layout.show_sha, "sha should still be shown here");
        assert!(!layout.show_age, "age should be the first thing dropped");
        assert!(layout.path_max_chars >= PATH_MIN_CHARS);
    }

    #[test]
    fn line2_layout_drops_sha_too_once_dropping_age_alone_is_not_enough() {
        let specs = [dirty_pill()];
        let layout = line2_layout(150.0, &specs, Some("abc1234"), Some("5mo"));
        assert!(!layout.show_sha);
        assert!(!layout.show_age);
        assert!(layout.path_max_chars >= PATH_ABSOLUTE_MIN_CHARS);
    }

    #[test]
    fn line2_layout_never_lets_the_path_disappear_even_under_extreme_pill_pressure() {
        // Three pills eating nearly the whole row and no sha/age left to
        // drop — the path still gets *something*, never zero, without this
        // function panicking on an underflow. Pills themselves are never
        // asked to shrink, down to a pathologically zero-width row.
        let specs = [dirty_pill(), gone_pill(), merged_pill()];
        let layout = line2_layout(175.0, &specs, None, None);
        assert!(!layout.show_sha);
        assert!(!layout.show_age);
        assert_eq!(layout.path_max_chars, PATH_ABSOLUTE_MIN_CHARS);

        let layout = line2_layout(0.0, &specs, Some("abc1234"), Some("5mo"));
        assert_eq!(layout.path_max_chars, PATH_ABSOLUTE_MIN_CHARS);
    }

    #[test]
    fn line1_max_chars_gives_the_branch_name_less_room_when_badge_and_lock_are_present() {
        let bare = line1_max_chars(300.0, false, false);
        let with_badge_and_lock = line1_max_chars(300.0, true, true);
        assert!(with_badge_and_lock < bare);
        assert!(with_badge_and_lock >= BRANCH_MIN_CHARS);
    }

    #[test]
    fn line1_max_chars_floors_at_branch_min_chars_when_the_row_is_pathologically_narrow() {
        assert_eq!(line1_max_chars(10.0, true, true), BRANCH_MIN_CHARS);
    }
}

#[cfg(test)]
mod sort_tests {
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
            dirty_count: 0,
            ahead: None,
            behind: None,
            upstream_gone: false,
            merged: false,
        }
    }

    fn dirty() -> WorktreeStatus {
        WorktreeStatus {
            dirty: true,
            dirty_count: 3,
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

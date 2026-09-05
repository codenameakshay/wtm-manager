//! The settings sheet: a modal, opened by the sidebar's gear button or ⌘,,
//! showing (and where honest, editing) everything the app remembers about
//! itself.
//!
//! Pure rendering plus one data table, in the spirit of [`crate::dialogs`]:
//! this module builds the sheet's content and wires the handful of clicks
//! that mutate [`crate::app::WtmApp`] state (appearance, dismissal); the
//! overlay's lifecycle (`self.settings_open`, mutual exclusion with
//! `self.dialog`, focus handling) lives in `crate::app`, the same split
//! every other overlay in this app uses.
//!
//! # What is and isn't editable here
//!
//! - **Appearance** is a real, persisted setting — a real `ui::segmented`
//!   control, writing straight through `WtmApp::set_appearance` to
//!   `prefs.json` via `on_select`, which is exactly the shape `cx.listener`
//!   produces.
//! - **Reduce motion** sits right below Appearance. It drives
//!   `WtmApp::set_reduce_motion` (mirrors `set_appearance`'s shape exactly:
//!   write `prefs.reduce_motion`, persist, then push the value to
//!   `motion::set_reduced` so the same render pass's `motion::reduced`
//!   read-back can never disagree with what was just toggled) and is fully
//!   persisted — `main.rs` applies `prefs.reduce_motion` at startup the same
//!   place it applies `prefs.appearance`.
//! - **Terminal app** is read-only. `Prefs::terminal` exists as a field, but
//!   `crate::data::open_in_terminal` only ever consults the `$WTM_TERMINAL`
//!   environment variable — never `Prefs::terminal`. An editable field here
//!   would silently do nothing when you tried to use it, which is worse than
//!   not offering one.
//! - **Effective repository configuration** is read-only by design: it is
//!   `wtm`'s own layered TOML config (see `wtm::config`), shared with the
//!   CLI and potentially checked into the repository. The app must never
//!   rewrite it, so the sheet only ever displays it, with a path and a
//!   "Reveal" button pointing at the real file. Both the global and repo
//!   config files are optional — plenty of installs have neither — so each
//!   path row checks `Path::exists` and says so plainly instead of implying
//!   a file is there to read; the sheet never offers to create one (`wtm
//!   config init` already does that for the repo config).
//! - **Keyboard shortcuts** are generated from [`crate::REGISTERED_BINDINGS`]
//!   (see `main.rs`'s `key_bindings!` macro) rather than hand-copied here,
//!   so this list cannot silently drift from what `cx.bind_keys` actually
//!   registers.

use std::path::PathBuf;

use gpui::prelude::*;
use gpui::{div, px, AnyElement, Context, ScrollHandle, SharedString};

use crate::app::WtmApp;
use crate::data::OpenRepo;
use crate::dialogs;
use crate::motion;
use crate::prefs::{Appearance, Prefs};
use crate::theme::{Theme, SPACE_12, SPACE_16, SPACE_2, SPACE_20, SPACE_4, SPACE_6, SPACE_8};
use crate::ui::{self, ButtonVariant, TEXT_SM, TEXT_XS};

/// One keyboard shortcut as shown in the "Keyboard Shortcuts" section. Built
/// by `main.rs`'s `key_bindings!` macro from the same list that registers
/// the real bindings, so the sheet can never drift from them.
pub struct ShortcutMeta {
    /// Human-facing glyph, e.g. `"⌘R"`.
    pub display: &'static str,
    /// What the binding does, e.g. `"Reload"`.
    pub label: &'static str,
}

/// Render the settings sheet: the scrim plus the card. Mirrors the treatment
/// `crate::app`'s dialog renderers give the create/remove/prune dialogs —
/// same backdrop, same card, same "click outside or Escape to dismiss".
pub fn render(
    prefs: &Prefs,
    repo: Option<&OpenRepo>,
    scroll: &ScrollHandle,
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> AnyElement {
    // SPACE_20 between sections: `better-layout`'s rule wants the gap
    // *between* groups at least 2x the gap *within* one (every section's
    // own internal gap below tops out at SPACE_12) — SPACE_20 clears that
    // floor. None of the four sections carry their own eyebrow label, so a
    // hairline `ui::divider` sits in that SPACE_20 gap between each pair to
    // make the boundary itself visible, not just wide.
    //
    // `.relative()` wrapper, sibling of the scrolling div — same reasoning
    // as `app::chrome`'s scroll regions (`ui::scrollbar`'s own doc): the
    // overlay must never be a descendant of the div it scrolls with.
    let scroll_region = div()
        .relative()
        .max_h(px(480.0))
        .child(
            div()
                .id("settings-body")
                .flex()
                .flex_col()
                .gap(px(SPACE_20))
                .px(px(SPACE_16))
                .py(px(SPACE_12))
                .max_h(px(480.0))
                .overflow_y_scroll()
                .track_scroll(scroll)
                .child(render_appearance_section(prefs.appearance, theme, cx))
                .child(ui::divider(theme))
                .child(render_terminal_section(theme))
                .child(ui::divider(theme))
                .child(render_config_section(repo, theme, cx))
                .child(ui::divider(theme))
                .child(render_shortcuts_section(theme)),
        )
        .child(ui::scrollbar(
            "settings-scrollbar",
            scroll,
            ui::ScrollAxis::Vertical,
        ));

    let body = div().flex().flex_col().child(scroll_region).child(
        ui::modal_footer(theme).child(
            ui::button("settings-done", "Done", ButtonVariant::Secondary, theme)
                .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
        ),
    );

    let card = ui::modal_card(480.0, theme)
        .id("settings-card")
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(ui::modal_header("Settings", None, theme))
        .child(body);

    // Same dialog-entrance treatment as every other modal surface:
    // `DIALOG_IN` on the card, `FADE_QUICK` on the scrim.
    let backdrop = ui::modal_backdrop()
        .id("settings-backdrop")
        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx)))
        .child(motion::dialog_in("settings-dialog-in", card, cx));
    motion::fade_quick("settings-dialog-backdrop-in", backdrop, cx).into_any_element()
}

// ---------------------------------------------------------------------
// Appearance
// ---------------------------------------------------------------------

fn render_appearance_section(
    current: Appearance,
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> AnyElement {
    let reduced = motion::reduced(cx);
    let options: [(Appearance, &str); 3] = [
        (Appearance::System, "System"),
        (Appearance::Light, "Light"),
        (Appearance::Dark, "Dark"),
    ];

    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_8))
        .child(ui::segmented(
            "appearance",
            &options,
            &current,
            theme,
            cx.listener(|this, value: &Appearance, window, cx| {
                this.set_appearance(*value, window, cx);
            }),
        ))
        .child(
            dialogs::render_toggle(
                "settings-reduce-motion",
                "Reduce motion",
                reduced,
                false,
                theme,
            )
            .on_click(cx.listener(|this, _, _window, cx| {
                let next = !motion::reduced(cx);
                this.set_reduce_motion(next, cx);
            })),
        )
        .child(dim_note(
            "Skips overlay and dialog entrance animations.",
            theme,
        ))
        .into_any_element()
}

// ---------------------------------------------------------------------
// Terminal
// ---------------------------------------------------------------------

fn render_terminal_section(theme: &Theme) -> impl IntoElement {
    let terminal = std::env::var("WTM_TERMINAL").unwrap_or_else(|_| "Terminal".to_string());

    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_4))
        .child(
            // Rendered as a value, not a heading: with the "Terminal App"
            // eyebrow gone, heading-weight text here would be the only thing
            // in the sheet that looked like a surviving section label. Match
            // the treatment `config_row` below uses for config values — mono
            // face, body size — so it reads as data, not a title.
            div()
                .font_family(ui::FONT_MONO)
                .text_size(px(TEXT_SM))
                .text_color(theme.text)
                .child(terminal),
        )
        .child(dim_note(
            "Set via the $WTM_TERMINAL environment variable — not editable here.",
            theme,
        ))
}

// ---------------------------------------------------------------------
// Effective repository configuration
// ---------------------------------------------------------------------

fn render_config_section(
    repo: Option<&OpenRepo>,
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> impl IntoElement {
    // No eyebrow names this group, and unlike Appearance or Terminal App, a
    // bare table of paths and values doesn't say what it is on its own — so
    // this note leads the section, doing double duty as both the "why you
    // can't edit this" caveat and the label telling you you're looking at
    // wtm's own repo config in the first place.
    let mut section = div().flex().flex_col().gap(px(SPACE_8)).child(dim_note(
        "Read-only — this is wtm's own TOML config; the app never rewrites it.",
        theme,
    ));

    section = match repo {
        None => section.child(dim_note(
            "Open a repository to see its effective configuration.",
            theme,
        )),
        Some(repo) => section
            .child(config_row(
                "Path template",
                repo.config.path_template.clone(),
                theme,
            ))
            .child(config_row(
                "Default base",
                repo.config
                    .default_base
                    .clone()
                    .unwrap_or_else(|| "HEAD".to_string()),
                theme,
            ))
            .child(config_row(
                "Editor",
                repo.config
                    .editor
                    .clone()
                    .unwrap_or_else(|| "$VISUAL / $EDITOR".to_string()),
                theme,
            ))
            .child(config_row(
                "Protected branches",
                if repo.config.prune.protected_branches.is_empty() {
                    "none".to_string()
                } else {
                    repo.config.prune.protected_branches.join(", ")
                },
                theme,
            ))
            .child(config_row(
                "Setup commands",
                if repo.config.setup.commands.is_empty() {
                    "none".to_string()
                } else {
                    repo.config.setup.commands.join("; ")
                },
                theme,
            ))
            .child(config_row(
                "Copy entries",
                if repo.config.setup.copy.is_empty() {
                    "none".to_string()
                } else {
                    repo.config
                        .setup
                        .copy
                        .iter()
                        .map(|entry| format!("{} ({:?})", entry.path, entry.mode))
                        .collect::<Vec<_>>()
                        .join(", ")
                },
                theme,
            )),
    };

    section.child(render_config_paths(repo, theme, cx))
}

/// The config file(s) the values above were merged from, each with a
/// "Reveal" button so the note at the top of this section ("read-only...
/// the app never rewrites it") points at something concrete rather than
/// asking the user to take it on faith.
fn render_config_paths(
    repo: Option<&OpenRepo>,
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> impl IntoElement {
    let mut rows = div().flex().flex_col().gap(px(SPACE_4));
    if let Some(global) = wtm::config::global_config_path() {
        rows = rows.child(config_path_row(
            "global-config",
            "Global config",
            global,
            "Not created — wtm uses its built-in defaults.",
            theme,
            cx,
        ));
    }
    if let Some(repo) = repo {
        let repo_config = repo.ctx.main_root.join(".worktree.toml");
        rows = rows.child(config_path_row(
            "repo-config",
            "Repo config",
            repo_config,
            "Not created — run `wtm config init` to add one.",
            theme,
            cx,
        ));
    }
    rows
}

/// Both config files are optional; on a fresh machine neither has to exist.
/// `reveal_in_finder` already handles a missing path by walking up to the
/// nearest existing ancestor, so this row only has to be honest about *this*
/// file's state: say plainly when it isn't there yet, and don't let "Reveal"
/// imply it opens a file that doesn't exist.
fn config_path_row(
    id: &'static str,
    label: &'static str,
    path: PathBuf,
    missing_note: &'static str,
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> impl IntoElement {
    let exists = path.exists();
    // Once the file is gone, "Reveal" would land on its containing folder
    // (via `reveal_in_finder`'s ancestor fallback) rather than the file
    // itself — say so instead of promising a file that isn't there.
    let button_label = if exists { "Reveal" } else { "Reveal Folder" };

    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(SPACE_8))
        .child(
            // `flex_1` is load-bearing: without it this column has no flex
            // basis of its own, so the row's flex-shrink negotiation (this
            // column vs. the `flex_none` button) can squeeze it toward zero
            // width, leaving `.truncate()` nothing to render but "…" — which
            // was exactly the bug. `flex_1` gives it a guaranteed claim on
            // (row width − button width − gap) instead, the same pattern
            // `ui::action_row`'s label column already uses.
            div()
                .flex_1()
                .flex()
                .flex_col()
                .min_w_0()
                .gap(px(SPACE_2))
                .child(
                    div()
                        .text_size(px(TEXT_SM))
                        .text_color(theme.text_muted)
                        .child(label),
                )
                .child(
                    // Paths take the bundled mono face. When the file
                    // doesn't exist there's no path to show — the caption
                    // below already says so in words — so this renders a
                    // plain em dash rather than a real (but nonexistent)
                    // path. `.truncate()` is deliberately dropped for that
                    // case: gpui 0.2.2's ellipsis truncation is unreliable,
                    // and a single glyph has nothing to truncate anyway —
                    // keeping `.truncate()` on it risks an "…"-only
                    // rendering for a missing file.
                    div()
                        .min_w_0()
                        .when(exists, |this| this.truncate())
                        .font_family(ui::FONT_MONO)
                        .text_size(px(TEXT_SM))
                        .text_color(theme.text)
                        .child(if exists {
                            crate::ui::display_path(&path)
                        } else {
                            "—".to_string()
                        }),
                )
                .when(!exists, |this| this.child(dim_note(missing_note, theme))),
        )
        .child(
            ui::button(
                SharedString::from(format!("reveal-{id}")),
                button_label,
                ButtonVariant::Ghost,
                theme,
            )
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.reveal_path_in_finder(path.clone(), cx);
            })),
        )
}

// ---------------------------------------------------------------------
// Keyboard shortcuts
// ---------------------------------------------------------------------

fn render_shortcuts_section(theme: &Theme) -> impl IntoElement {
    div().flex().flex_col().gap(px(SPACE_6)).children(
        crate::REGISTERED_BINDINGS
            .iter()
            .map(|entry| shortcut_row(entry, theme)),
    )
}

fn shortcut_row(entry: &ShortcutMeta, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(SPACE_8))
        .child(
            div()
                .text_size(px(TEXT_SM))
                .text_color(theme.text)
                .child(entry.label),
        )
        .child(ui::kbd(entry.display, theme))
}

// ---------------------------------------------------------------------
// Small shared pieces
// ---------------------------------------------------------------------

fn dim_note(text: &'static str, theme: &Theme) -> impl IntoElement {
    div()
        .text_size(px(TEXT_XS))
        .text_color(theme.text_ghost)
        .child(text)
}

/// The detail panel's fact-list treatment (a fixed label column, `text_muted`
/// at `TEXT_SM`, full-strength `text` for the value), reused here since every
/// row this renders
/// (a path template, a ref name, a joined list of shell commands) is exactly
/// that same shape. Every value is config-file content, so it takes
/// [`ui::FONT_MONO`] across the board rather than picking case by case which
/// of these reads as a "path" or a "ref".
fn config_row(
    label: &'static str,
    value: impl Into<SharedString>,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .items_baseline()
        .gap(px(SPACE_8))
        .child(
            div()
                .flex_none()
                .w(px(130.0))
                .text_size(px(TEXT_SM))
                .text_color(theme.text_muted)
                .child(label),
        )
        .child(
            // `flex_1` (not just `min_w_0`) so a long value — a joined list
            // of setup commands or copy entries — reliably claims the row's
            // full remaining width to wrap into, instead of a size the flex
            // shrink negotiation happens to leave it. Same collapsed-column
            // mistake as `config_path_row` had, just without `.truncate()`
            // to make the symptom as visible.
            div()
                .flex_1()
                .min_w_0()
                .font_family(ui::FONT_MONO)
                .text_size(px(TEXT_SM))
                .text_color(theme.text)
                .child(value.into()),
        )
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_registered_binding_has_a_display_and_label() {
        assert!(
            !crate::REGISTERED_BINDINGS.is_empty(),
            "the shortcuts list should not be empty"
        );
        for entry in crate::REGISTERED_BINDINGS {
            assert!(!entry.display.is_empty());
            assert!(!entry.label.is_empty());
        }
    }

    #[test]
    fn registered_bindings_match_what_main_rs_actually_binds() {
        // `key_bindings!` (in `main.rs`) generates `registered_key_bindings`
        // (the real `Vec<KeyBinding>` handed to `cx.bind_keys`) and
        // `REGISTERED_BINDINGS` (this display metadata) from one macro
        // invocation, so they cannot drift in content — but this still
        // guards the shape of that macro expansion itself.
        assert_eq!(
            crate::registered_key_bindings().len(),
            crate::REGISTERED_BINDINGS.len(),
            "every registered KeyBinding should have exactly one metadata entry"
        );
    }
}

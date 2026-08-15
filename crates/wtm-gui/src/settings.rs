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
//! - **Appearance** is a real, persisted setting — the segmented control
//!   writes straight through `WtmApp::set_appearance` to `prefs.json`.
//! - **Terminal app** is read-only. `Prefs::terminal` exists as a field, but
//!   `crate::data::open_in_terminal` (owned elsewhere, not part of this
//!   task) only ever consults the `$WTM_TERMINAL` environment variable —
//!   never `Prefs::terminal`. An editable field here would silently do
//!   nothing when you tried to use it, which is worse than not offering one;
//!   see the module-level task notes for why this is flagged as a gap in
//!   `data.rs` rather than worked around.
//! - **Effective repository configuration** is read-only by design: it is
//!   `wtm`'s own layered TOML config (see `wtm::config`), shared with the
//!   CLI and potentially checked into the repository. The app must never
//!   rewrite it, so the sheet only ever displays it, with a path and a
//!   "Reveal" button pointing at the real file.
//! - **Keyboard shortcuts** are generated from [`crate::REGISTERED_BINDINGS`]
//!   (see `main.rs`'s `key_bindings!` macro) rather than hand-copied here,
//!   so this list cannot silently drift from what `cx.bind_keys` actually
//!   registers.

use std::path::PathBuf;

use gpui::prelude::*;
use gpui::{div, hsla, px, AnyElement, Context, FontWeight, SharedString, Stateful};

use crate::app::WtmApp;
use crate::data::OpenRepo;
use crate::prefs::{Appearance, Prefs};
use crate::theme::Theme;
use crate::ui::{self, ButtonVariant};

/// One keyboard shortcut, as registered with gpui and as shown in the
/// "Keyboard Shortcuts" section below. Built by `main.rs`'s `key_bindings!`
/// macro from a single list, so `keystroke` here is always exactly what
/// `cx.bind_keys` used to register the real binding.
pub struct ShortcutMeta {
    /// gpui keystroke syntax, e.g. `"cmd-r"` — the value actually passed to
    /// `KeyBinding::new`. Not rendered directly (see `display`); kept so a
    /// test can assert this table's shape without duplicating the real
    /// keystroke strings by hand.
    #[allow(dead_code)]
    pub keystroke: &'static str,
    /// The key context the binding is scoped to (`None` for a window-global
    /// binding like Quit). Kept for completeness / future filtering; not
    /// currently rendered.
    #[allow(dead_code)]
    pub context: Option<&'static str>,
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
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> AnyElement {
    let body = div()
        .id("settings-body")
        .flex()
        .flex_col()
        .gap(px(20.0))
        .px(px(16.0))
        .py(px(14.0))
        .max_h(px(480.0))
        .overflow_y_scroll()
        .child(render_appearance_section(prefs.appearance, theme, cx))
        .child(render_terminal_section(theme))
        .child(render_config_section(repo, theme, cx))
        .child(render_shortcuts_section(theme))
        .child(
            ui::modal_footer(theme).child(
                ui::button("settings-done", "Done", ButtonVariant::Secondary, theme)
                    .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
            ),
        );

    ui::modal_backdrop()
        .id("settings-backdrop")
        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx)))
        .child(
            ui::modal_card(480.0, theme)
                .id("settings-card")
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(ui::modal_header("Settings", None, theme))
                .child(body),
        )
        .into_any_element()
}

// ---------------------------------------------------------------------
// Appearance
// ---------------------------------------------------------------------

fn render_appearance_section(
    current: Appearance,
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(section_label("Appearance", theme))
        .child(
            div()
                .flex()
                .gap(px(6.0))
                .child(appearance_option(
                    Appearance::System,
                    "System",
                    current,
                    theme,
                    cx,
                ))
                .child(appearance_option(
                    Appearance::Light,
                    "Light",
                    current,
                    theme,
                    cx,
                ))
                .child(appearance_option(
                    Appearance::Dark,
                    "Dark",
                    current,
                    theme,
                    cx,
                )),
        )
}

fn appearance_option(
    value: Appearance,
    label: &'static str,
    current: Appearance,
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> Stateful<gpui::Div> {
    let selected = value == current;
    // Mid-tone in both palettes (mirrors `ui::button`'s own
    // `dark_foreground`, private to that module), so the selected pill's
    // label stays legible against `theme.accent` in either theme.
    let dark_foreground = hsla(0.0, 0.0, 0.08, 1.0);

    div()
        .id(SharedString::from(format!("appearance-{label}")))
        .px(px(12.0))
        .h(px(26.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(ui::RADIUS))
        .cursor_default()
        .text_size(px(12.0))
        .when(selected, |this| {
            this.bg(theme.accent).text_color(dark_foreground)
        })
        .when(!selected, |this| {
            this.bg(theme.item_wash)
                .text_color(theme.text)
                .hover(|s| s.bg(theme.item_selected))
        })
        .child(label)
        .on_click(cx.listener(move |this, _, window, cx| {
            this.set_appearance(value, window, cx);
        }))
}

// ---------------------------------------------------------------------
// Terminal
// ---------------------------------------------------------------------

fn render_terminal_section(theme: &Theme) -> impl IntoElement {
    let terminal = std::env::var("WTM_TERMINAL").unwrap_or_else(|_| "Terminal".to_string());

    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(section_label("Terminal App", theme))
        .child(
            div()
                .text_size(px(12.5))
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
    let mut section = div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(section_label("Effective Repository Configuration", theme));

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

    section
        .child(dim_note(
            "Read-only — this is wtm's own TOML config; the app never rewrites it.",
            theme,
        ))
        .child(render_config_paths(repo, theme, cx))
}

/// The config file(s) the values above were merged from, each with a
/// "Reveal" button so the note above ("read-only... the app never rewrites
/// it") points at something concrete rather than asking the user to take it
/// on faith.
fn render_config_paths(
    repo: Option<&OpenRepo>,
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> impl IntoElement {
    let mut rows = div().flex().flex_col().gap(px(4.0));
    if let Some(global) = wtm::config::global_config_path() {
        rows = rows.child(config_path_row(
            "global-config",
            "Global config",
            global,
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
            theme,
            cx,
        ));
    }
    rows
}

fn config_path_row(
    id: &'static str,
    label: &'static str,
    path: PathBuf,
    theme: &Theme,
    cx: &mut Context<WtmApp>,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .gap(px(1.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.text_tertiary)
                        .child(label),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.5))
                        .text_color(theme.text_secondary)
                        .child(path.display().to_string()),
                ),
        )
        .child(
            ui::button(
                SharedString::from(format!("reveal-{id}")),
                "Reveal",
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
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(section_label("Keyboard Shortcuts", theme))
        .children(
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
        .gap(px(8.0))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(theme.text)
                .child(entry.label),
        )
        .child(ui::kbd(entry.display, theme))
}

// ---------------------------------------------------------------------
// Small shared pieces
// ---------------------------------------------------------------------

fn section_label(label: &'static str, theme: &Theme) -> impl IntoElement {
    div()
        .text_size(px(12.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text_secondary)
        .child(label)
}

fn dim_note(text: &'static str, theme: &Theme) -> impl IntoElement {
    div()
        .text_size(px(11.0))
        .text_color(theme.text_ghost)
        .child(text)
}

fn config_row(
    label: &'static str,
    value: impl Into<SharedString>,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .items_baseline()
        .gap(px(10.0))
        .child(
            div()
                .flex_none()
                .w(px(130.0))
                .text_size(px(11.5))
                .text_color(theme.text_tertiary)
                .child(label),
        )
        .child(
            div()
                .min_w_0()
                .text_size(px(12.0))
                .text_color(theme.text_secondary)
                .child(value.into()),
        )
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_registered_binding_has_a_keystroke_and_label() {
        assert!(
            !crate::REGISTERED_BINDINGS.is_empty(),
            "the shortcuts list should not be empty"
        );
        for entry in crate::REGISTERED_BINDINGS {
            assert!(!entry.keystroke.is_empty());
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

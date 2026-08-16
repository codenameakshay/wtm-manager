//! The app's UI vocabulary: the handful of primitives every screen is built
//! from.
//!
//! Keeping these in one place is what makes the interface feel like one
//! product — a row in the sidebar and a row in the worktree list share the
//! same height, radius, and hover wash because they are literally built from
//! the same functions.

use gpui::prelude::*;
use gpui::{div, hsla, px, svg, Div, FontWeight, Hsla, SharedString, Stateful};

use crate::theme::Theme;

/// Corner radius for every interactive row and button.
pub const RADIUS: f32 = 7.0;
/// Height of a single-line action row (New Worktree, Search).
pub const ROW_HEIGHT: f32 = 32.0;
/// Height of the window's title bar strip.
pub const TITLEBAR_HEIGHT: f32 = 48.0;
/// Horizontal room the macOS traffic lights need before content may start.
pub const TRAFFIC_LIGHT_CLEARANCE: f32 = 78.0;

/// An icon, sized in pixels and tinted.
///
/// Icons are square and inherit no color of their own, so the caller always
/// states the tint — that is what keeps a disabled row's icon from staying
/// bright while its label dims.
pub fn icon(path: &'static str, size: f32, color: Hsla) -> impl IntoElement {
    svg()
        .path(path)
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// A square icon button, as used in the title bar.
pub fn icon_button(
    id: impl Into<gpui::ElementId>,
    path: &'static str,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .w(px(26.0))
        .h(px(26.0))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor_default()
        .hover(|this| this.bg(theme.item_wash))
        .child(icon(path, 14.0, theme.text_tertiary))
}

/// A small uppercase-free section label, e.g. "Repositories".
pub fn section_header(label: impl Into<SharedString>, theme: &Theme) -> impl IntoElement {
    div()
        .h(px(28.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .text_size(px(12.5))
        .text_color(theme.text_tertiary)
        .child(label.into())
}

/// The base of every selectable row: fixed radius, one neutral wash for
/// hover and selection, no per-state hues.
pub fn row(id: impl Into<gpui::ElementId>, selected: bool, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id.into())
        .w_full()
        .min_w_0()
        .px(px(8.0))
        .py(px(7.0))
        .rounded(px(RADIUS))
        .cursor_default()
        .when(selected, |this| this.bg(theme.item_selected))
        .when(!selected, |this| this.hover(|s| s.bg(theme.item_wash)))
}

/// A single-line action row: icon, label, optional trailing shortcut hint.
pub fn action_row(
    id: impl Into<gpui::ElementId>,
    path: &'static str,
    label: impl Into<SharedString>,
    shortcut: Option<&str>,
    theme: &Theme,
) -> Stateful<Div> {
    let shortcut = shortcut.map(|s| s.to_string());

    div()
        .id(id.into())
        .h(px(ROW_HEIGHT))
        .w_full()
        .px(px(8.0))
        .flex()
        .items_center()
        .gap(px(10.0))
        .rounded(px(RADIUS))
        .cursor_default()
        .hover(|this| this.bg(theme.item_wash))
        .child(icon(path, 15.0, theme.text_secondary))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.0))
                .text_color(theme.text)
                .child(label.into()),
        )
        .when_some(shortcut, |this, keys| {
            this.child(
                div()
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(theme.text_ghost)
                    .child(keys),
            )
        })
}

/// A meta item for a row's second line: a small icon and a muted label.
pub fn meta(path: &'static str, label: impl Into<SharedString>, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .min_w_0()
        .items_center()
        .gap(px(5.0))
        .child(icon(path, 11.0, theme.text_ghost))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(theme.text_tertiary)
                .child(label.into()),
        )
}

/// A status pill: a colored dot and a label, tinted by meaning.
///
/// The dot carries the color and the text stays legible, which keeps a row
/// with several pills from turning into a rainbow.
pub fn pill(label: impl Into<SharedString>, color: Hsla) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.0))
        .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(color))
        .child(div().text_color(color).child(label.into()))
}

/// The four button treatments a dialog needs: one to commit ("Create"), one
/// for the alternate action next to it ("Cancel"), one for something
/// destructive ("Delete"), and one quiet enough to sit in a toolbar without
/// competing with the content around it.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Wired up once the first dialog lands on top of TextInput.
pub enum ButtonVariant {
    Primary,
    Secondary,
    Danger,
    Ghost,
}

/// Darken or lighten a color by shifting lightness only, so hover/active
/// feedback on a solid-colored button reads as "the same color, pressed"
/// rather than "a different, half-transparent color" — which is what
/// adjusting alpha over a variable backdrop would look like instead.
fn shade(color: Hsla, delta: f32) -> Hsla {
    Hsla {
        l: (color.l + delta).clamp(0.0, 1.0),
        ..color
    }
}

/// A labeled button. The caller attaches `.on_click(...)`; this function
/// only owns the look and the hover/press feedback, the same split as
/// [`row`] and [`action_row`] above.
#[allow(dead_code)] // Wired up once the first dialog needs a Create/Cancel/Delete action.
pub fn button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    variant: ButtonVariant,
    theme: &Theme,
) -> Stateful<Div> {
    // Accent and danger are mid-tone in both palettes, so a fixed dark
    // foreground stays legible on either — reading `theme.text` here would
    // turn near-white in dark mode and wash out against the orange.
    let dark_foreground = hsla(0.0, 0.0, 0.08, 1.0);

    let (bg, bg_hover, bg_active, text_color) = match variant {
        ButtonVariant::Primary => (
            theme.accent,
            shade(theme.accent, -0.05),
            shade(theme.accent, -0.10),
            dark_foreground,
        ),
        ButtonVariant::Secondary => (
            theme.item_wash,
            theme.item_selected,
            theme.item_selected,
            theme.text,
        ),
        ButtonVariant::Danger => (
            theme.danger,
            shade(theme.danger, -0.05),
            shade(theme.danger, -0.10),
            dark_foreground,
        ),
        ButtonVariant::Ghost => (
            gpui::transparent_black(),
            theme.item_wash,
            theme.item_selected,
            theme.text,
        ),
    };

    div()
        .id(id.into())
        .h(px(28.0))
        .px(px(12.0))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS))
        .cursor_default()
        .bg(bg)
        .text_size(px(12.5))
        .text_color(text_color)
        .hover(|this| this.bg(bg_hover))
        .active(|this| this.bg(bg_active))
        .child(label.into())
}

/// A small keyboard-shortcut badge, for next to a button's label. Distinct
/// from the shortcut hint [`action_row`] prints inline: that one sits in a
/// list and stays plain text, this one sits next to a button and needs to
/// read as its own small chip.
#[allow(dead_code)] // Wired up once a dialog wants a shortcut next to a button.
pub fn kbd(keys: &str, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .px(px(5.0))
        .rounded(px(4.0))
        .bg(theme.item_wash)
        .text_size(px(11.0))
        .text_color(theme.text_ghost)
        .child(keys.to_string())
}

/// A full-window scrim behind a modal, centering whatever dialog sits on
/// top of it. The caller stacks this as a top-level, absolutely-positioned
/// child — it is not meant to participate in normal layout flow.
#[allow(dead_code)] // Wired up once the first modal dialog lands.
pub fn modal_backdrop() -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(hsla(0.0, 0.0, 0.0, 0.45))
}

/// The dialog surface itself: a raised panel with enough shadow to read as
/// floating above the scrim, at a fixed width so its content doesn't
/// reflow while its data loads.
#[allow(dead_code)] // Wired up once the first modal dialog lands.
pub fn modal_card(width: f32, theme: &Theme) -> Div {
    div()
        .w(px(width))
        .flex()
        .flex_col()
        .bg(theme.raised)
        .border_1()
        .border_color(theme.border_strong)
        .rounded(px(12.0))
        .shadow_lg()
}

/// A modal's title, with an optional supporting line beneath it.
#[allow(dead_code)] // Wired up once the first modal dialog lands.
pub fn modal_header(
    title: impl Into<SharedString>,
    subtitle: Option<&str>,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .px(px(16.0))
        .pt(px(16.0))
        .child(
            div()
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(title.into()),
        )
        .when_some(subtitle.map(str::to_string), |this, subtitle| {
            this.child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.text_tertiary)
                    .child(subtitle),
            )
        })
}

/// A right-aligned button row along a modal's bottom edge, separated from
/// the body by a hairline so it reads as the dialog's fixed action bar even
/// when the content above it scrolls.
#[allow(dead_code)] // Wired up once the first modal dialog lands.
pub fn modal_footer(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_end()
        .gap(px(8.0))
        .px(px(16.0))
        .py(px(12.0))
        .border_t_1()
        .border_color(theme.border)
}

/// Centered, muted helper text for an empty state inside a panel — the
/// "Run `wtm` inside a repository…" line's shape, generalized so other
/// panels don't have to hand-roll it.
#[allow(dead_code)] // Wired up once another panel needs an empty state.
pub fn empty_hint(text: impl Into<SharedString>, theme: &Theme) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.5))
        .text_color(theme.text_tertiary)
        .child(text.into())
}

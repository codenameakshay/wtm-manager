//! The app's UI vocabulary: the handful of primitives every screen is built
//! from.
//!
//! Keeping these in one place is what makes the interface feel like one
//! product — a row in the sidebar and a row in the worktree list share the
//! same height, radius, and hover wash because they are literally built from
//! the same functions.
//!
//! # Rules every component here follows
//!
//! 1. **A component owns its look and its feedback; the caller owns
//!    behaviour.** Components return `Stateful<Div>` (or `impl IntoElement`)
//!    and the caller attaches `.on_click(..)` — no component here takes a
//!    click handler as a parameter. The one function that bends this rule is
//!    [`segmented`]; see its doc for why.
//! 2. **No component reads `cx` for colors.** Every component takes `&Theme`
//!    explicitly.
//! 3. **Radii are concentric.** Nested containers hardcode the nearest
//!    `RADIUS_*` constant rather than eyeballing a radius.
//! 4. **Every interactive component has rest/hover/active/focused-or-selected
//!    states**, and none of them is signalled by motion alone.
//! 5. **Icon stroke matches text weight** — the set is Lucide 24×24
//!    stroke-2 throughout; nothing here introduces a second stroke weight.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

use gpui::prelude::*;
use gpui::{
    div, linear_color_stop, linear_gradient, point, px, svg, AnyElement, AnyView, App, Context,
    Div, ElementId, Font, FontWeight, Hsla, Render, ScrollHandle, SharedString, Stateful, Window,
};

use crate::motion;
use crate::theme::{
    hairline, scrim, Theme, ICON_BUTTON_SIZE, RADIUS_CHIP, RADIUS_CONTROL, RADIUS_DIALOG,
    RADIUS_PANEL, RADIUS_ROW, SCRIM_ALPHA_DARK, SPACE_12, SPACE_16, SPACE_2, SPACE_32, SPACE_4,
    SPACE_6, SPACE_8,
};

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

/// Height of a single-line action row (New Worktree, Search). Re-exports
/// [`crate::theme::ROW_HEIGHT`] so every `ui::ROW_HEIGHT` call site keeps
/// reading from one source of truth.
pub const ROW_HEIGHT: f32 = crate::theme::ROW_HEIGHT;
/// Height of the window's title bar strip. Re-exports
/// [`crate::theme::TITLEBAR_HEIGHT`].
pub const TITLEBAR_HEIGHT: f32 = crate::theme::TITLEBAR_HEIGHT;
/// Horizontal room the macOS traffic lights need before content may start.
/// Re-exports [`crate::theme::TRAFFIC_LIGHT_CLEARANCE`].
pub const TRAFFIC_LIGHT_CLEARANCE: f32 = crate::theme::TRAFFIC_LIGHT_CLEARANCE;

/// Text scale, re-exported from `theme.rs` under the same names so every
/// `ui::TEXT_*` call site reads from one source of truth.
pub const TEXT_XS: f32 = crate::theme::TEXT_XS;
pub const TEXT_SM: f32 = crate::theme::TEXT_SM;
pub const TEXT_BASE: f32 = crate::theme::TEXT_BASE;
pub const TEXT_MD: f32 = crate::theme::TEXT_MD;
pub const TEXT_LG: f32 = crate::theme::TEXT_LG;
pub const TEXT_XL: f32 = crate::theme::TEXT_XL;

/// The bundled monospace family (paths, SHAs, branch names, diff content).
/// Re-exports [`crate::theme::FONT_MONO_DEFAULT`] as a bare `&str` for call
/// sites that reach for it without a `&Theme` in scope (e.g. [`mono_font`]
/// below); [`meta`]/[`kbd`] prefer `theme.font_mono` directly since they
/// already have a `&Theme`.
pub const FONT_MONO: &str = crate::theme::FONT_MONO_DEFAULT;

/// Platform monospace fallbacks, tried in order after [`FONT_MONO`] if the
/// bundled face fails to register.
const MONOSPACE_FALLBACKS: &[&str] = &["SF Mono", "Menlo", "Monaco", "Courier New"];

/// The app's one monospace font, for every surface that renders code, diffs,
/// or command output (paths/SHAs/branch names use `theme.font_mono`
/// directly instead, since they already have a `&Theme` in scope).
pub fn mono_font() -> Font {
    let mut f = gpui::font(FONT_MONO);
    f.fallbacks = Some(gpui::FontFallbacks::from_fonts(
        MONOSPACE_FALLBACKS.iter().map(|s| s.to_string()).collect(),
    ));
    f
}

/// Estimated advance width, in pixels, of one monospace character at the UI
/// font's `TEXT_SM` size (~0.6em of the 12px UI font) — used to size columns
/// (a diff gutter, a truncation budget) without a real shaped-text layout
/// pass, which gpui has no API to measure outside of one.
pub const CHAR_WIDTH_APPROX: f32 = 7.2;

/// The user's home directory, read once and cached for [`display_path`].
static HOME: OnceLock<Option<PathBuf>> = OnceLock::new();

fn home_dir() -> Option<&'static Path> {
    HOME.get_or_init(|| std::env::var_os("HOME").map(PathBuf::from))
        .as_deref()
}

/// Home-relative display form of `path`, e.g. `~/code/project`.
pub fn display_path(path: &Path) -> String {
    wtm::output::abbreviate_path(path, home_dir())
}

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

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

/// A 1px hairline divider, full width. Structural separators only (pane
/// boundaries, a dialog's action bar) — per `better-layout` §1, separators
/// *inside* a pane are mostly deleted in favor of spacing.
pub fn divider(theme: &Theme) -> Div {
    div().w_full().h(px(1.0)).flex_none().bg(theme.border)
}

/// `text`, or a single space if `text` is empty — an empty line in a log or
/// diff body still needs to occupy its line height, which a genuinely empty
/// text child does not reliably do.
pub fn non_empty_or_space(text: &str) -> &str {
    if text.is_empty() {
        " "
    } else {
        text
    }
}

// ---------------------------------------------------------------------------
// Focus and press
// ---------------------------------------------------------------------------

/// The accent focus ring: a real border, never a drop shadow (a drop shadow
/// paints *behind* the element and would show through any translucent fill
/// as a grey plate). gpui's border draws inward from the box edge — there is
/// no `inset` box-shadow field to reach for — which already gives the "1px
/// inner offset" feel the ring wants without a genuine negative-offset
/// primitive.
///
/// Generic over [`Styled`] so the same implementation composes both with
/// `.when(selected, focus_ring(theme))` for a statically-known selection
/// flag (`Div`/
/// `Stateful<Div>` both implement `Styled`) and with gpui's own live
/// `.focus(focus_ring(theme))` / `.in_focus(..)` pseudo-classes, which
/// operate on `StyleRefinement` — verified against gpui-0.2.2's
/// `elements/div.rs`: a literal `Fn(Div) -> Div` cannot be passed to either
/// of gpui's own state builders, only to `.when`.
pub fn focus_ring<E: Styled>(theme: &Theme) -> impl Fn(E) -> E {
    let accent = theme.accent;
    move |el: E| el.border_2().border_color(accent)
}

/// Press feedback for elements whose rest state is already translucent
/// (icon buttons, rows, ghost buttons): swaps in `theme.element_active` and
/// nudges 1px inward via [`motion::press_feedback`] — see that function's
/// doc for why this stands in for `scale(0.96)` in a framework with no div
/// scale transform.
///
/// **Not** correct for an opaque colored plate: an opaque plate
/// (`surface_raised` and friends) should *brighten*, never swap to a
/// translucent wash. [`button`]'s `Primary`/`Secondary`/`Danger` variants
/// rest on exactly such a plate, so they use a private `shade`-based press
/// instead of this helper; only their `Ghost` variant (genuinely
/// transparent at rest) calls this function.
pub fn press_feedback(el: Stateful<Div>, theme: &Theme) -> Stateful<Div> {
    let active_wash = theme.element_active;
    el.active(move |style| motion::press_feedback(style, active_wash))
}

/// Marks an already-built [`row`]/[`button`]/[`icon_button`]/[`toolbar_button`]
/// as unavailable: removes it from keyboard reach via gpui's own
/// `InteractiveElement::tab_stop(false)` (its doc: "the element remains in
/// tab-index order but cannot be reached via keyboard navigation" — verified
/// against `gpui-0.2.2/src/elements/div.rs`), without touching the slot the
/// component already claimed in the tab order, so disabling a control never
/// reshuffles the sequence around it.
///
/// A harden-pass finding: an unavailable Prune/Create/Remove/Run confirm
/// button and a checked-out branch row were all still real Tab stops — they
/// painted dim (or, for the branch row, just never got a click handler) but
/// Tab still landed on them and Enter/Space silently did nothing. Every one
/// of those call sites built its own "does this control actually have an
/// action" branch already (`if can_submit { .. } else { button.opacity(0.4)
/// }`); this is the component-layer half those branches were missing, not a
/// new per-component `disabled: bool` parameter — a component owns its look
/// while the caller owns behaviour, and the call site is the only place
/// that already knows whether there's a click handler behind a control.
///
/// Visual dimming stays the call site's job too: the button call sites above
/// already chain `.opacity(0.4)`, and [`crate::dialogs::render_branch_row`]
/// mutes its own text color directly — this function only owns the focus
/// half.
pub fn disabled(el: Stateful<Div>) -> Stateful<Div> {
    el.tab_stop(false)
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// The base of every selectable row: `RADIUS_ROW`, one neutral wash for
/// hover, a stronger one for selection. Selection reads from the wash alone
/// (see `element_active`'s doc in `theme.rs` for how it stays
/// distinguishable from `element_hover`), with the focus ring handling
/// keyboard focus — a coloured left border on a list item is decoration,
/// not signal.
pub fn row(id: impl Into<gpui::ElementId>, selected: bool, theme: &Theme) -> Stateful<Div> {
    let styled = div()
        .id(id.into())
        .relative()
        .w_full()
        .min_w_0()
        .px(px(SPACE_8))
        .py(px(SPACE_6))
        .rounded(px(RADIUS_ROW))
        .cursor_default()
        .when(selected, |this| this.bg(theme.element_active))
        .when(!selected, |this| this.hover(|s| s.bg(theme.element_hover)))
        // Gated on `theme.tab_stops` — see that field's doc — so this stays
        // out of the tab order for the background shell while a dialog
        // covers it.
        .when(theme.tab_stops, |this| this.tab_index(0))
        .focus(focus_ring(theme));

    press_feedback(styled, theme)
}

/// A single-line action row: icon, label, optional trailing shortcut hint.
/// The shortcut renders as a [`kbd`] chip rather than plain text — the
/// sidebar's "New Worktree"/"Search" rows are what this exists for.
pub fn action_row(
    id: impl Into<gpui::ElementId>,
    path: &'static str,
    label: impl Into<SharedString>,
    shortcut: Option<&str>,
    theme: &Theme,
) -> Stateful<Div> {
    let shortcut = shortcut.map(|s| s.to_string());

    let styled = div()
        .id(id.into())
        .h(px(ROW_HEIGHT))
        .w_full()
        .px(px(SPACE_8))
        .flex()
        .items_center()
        .gap(px(SPACE_8))
        .rounded(px(RADIUS_ROW))
        .cursor_default()
        .hover(|this| this.bg(theme.element_hover))
        .when(theme.tab_stops, |this| this.tab_index(0))
        .focus(focus_ring(theme))
        .child(icon(path, 15.0, theme.text_muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(TEXT_BASE))
                .text_color(theme.text)
                .child(label.into()),
        )
        .when_some(shortcut, |this, keys| this.child(kbd(&keys, theme)));

    press_feedback(styled, theme)
}

/// A meta item for a row's second line: a small icon and a muted label.
/// Paths, SHAs and branch names are exactly what this renders in practice,
/// so the label always takes `theme.font_mono` — a SHA in a proportional
/// face is a small but constant readability tax.
pub fn meta(path: &'static str, label: impl Into<SharedString>, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .min_w_0()
        .items_center()
        .gap(px(SPACE_4))
        .child(icon(path, 11.0, theme.text_ghost))
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_family(theme.font_mono)
                .text_size(px(TEXT_SM))
                .text_color(theme.text_muted)
                .child(label.into()),
        )
}

// ---------------------------------------------------------------------------
// Chips
// ---------------------------------------------------------------------------

/// A status pill: a colored dot and a colored label, on no plate of its own.
/// The *dot* carries the color; the containing row/card is the neutral
/// plate. `pill` intentionally takes no `&Theme` — it paints no background —
/// which is what keeps a row with three pills from turning into a rainbow
/// of little tinted plates instead of one neutral row with three colored
/// dots.
pub fn pill(label: impl Into<SharedString>, color: Hsla) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(SPACE_4))
        .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(color))
        .child(
            div()
                .text_size(px(TEXT_SM))
                .text_color(color)
                .child(label.into()),
        )
}

/// The `main` branch badge. Unlike [`pill`], it paints its own
/// `surface_raised` plate: it is meant to stand alone rather than sit inside
/// an already-neutral row.
pub fn main_badge(theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(16.0))
        .px(px(SPACE_6))
        .rounded(px(RADIUS_CHIP))
        .bg(theme.surface_raised)
        .text_size(px(TEXT_XS))
        .text_color(theme.text_muted)
        .child("main")
}

/// A small keyboard-shortcut badge, for next to a button's label or inside
/// an [`action_row`]. Distinct from a bare shortcut string: this renders as
/// its own small chip.
pub fn kbd(keys: &str, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(16.0))
        .px(px(SPACE_6))
        .rounded(px(RADIUS_CHIP))
        .bg(theme.surface_raised)
        .border_1()
        .border_color(theme.border)
        .font_family(theme.font_mono)
        .text_size(px(TEXT_XS))
        .text_color(theme.text_ghost)
        .child(keys.to_string())
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

/// The four button treatments a dialog needs: one to commit ("Create"), one
/// for the alternate action next to it ("Cancel"), one for something
/// destructive ("Delete"), and one quiet enough to sit in a toolbar without
/// competing with the content around it.
#[derive(Clone, Copy, PartialEq, Eq)]
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

/// Rest/hover background and label color for each [`ButtonVariant`], shared
/// by [`button`] and [`toolbar_button`] so the two stay visually identical
/// apart from the leading icon. `Primary`/`Danger` carry `on_accent` — never
/// `theme.text`, which goes near-white in dark mode and washes out on a
/// colored plate.
fn button_colors(variant: ButtonVariant, theme: &Theme) -> (Hsla, Hsla, Hsla) {
    match variant {
        ButtonVariant::Primary => (
            theme.accent_strong,
            shade(theme.accent_strong, -0.06),
            theme.on_accent,
        ),
        ButtonVariant::Secondary => (theme.surface_raised, theme.surface_raised_hover, theme.text),
        ButtonVariant::Danger => (
            theme.danger_strong,
            shade(theme.danger_strong, -0.06),
            theme.on_accent,
        ),
        ButtonVariant::Ghost => (gpui::transparent_black(), theme.element_hover, theme.text),
    }
}

/// Press feedback for a button: an opaque plate should brighten, never swap
/// to a translucent wash, so `Primary`/`Secondary`/`Danger` rest on an
/// opaque fill and their press state is a
/// further [`shade`] of that same fill plus a 1px inset — the same
/// mechanic [`press_feedback`] uses, without the wash swap that would be
/// wrong here. Only `Ghost` rests on a genuinely transparent fill, where
/// [`press_feedback`] (this module's public helper) is correct as-is.
fn button_press(
    el: Stateful<Div>,
    variant: ButtonVariant,
    rest_bg: Hsla,
    theme: &Theme,
) -> Stateful<Div> {
    match variant {
        ButtonVariant::Ghost => press_feedback(el, theme),
        _ => {
            let active_bg = shade(rest_bg, -0.10);
            el.active(move |style| style.bg(active_bg).relative().top(px(1.0)))
        }
    }
}

/// A labeled button. The caller attaches `.on_click(...)`; this function
/// only owns the look and the hover/press feedback, the same split as
/// [`row`] and [`action_row`] above.
///
/// Exactly one `Primary` belongs in any single view — that is a call-site
/// discipline, not something this function can enforce.
pub fn button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    variant: ButtonVariant,
    theme: &Theme,
) -> Stateful<Div> {
    let (bg, bg_hover, text_color) = button_colors(variant, theme);
    let styled = div()
        .id(id.into())
        .h(px(28.0))
        .px(px(SPACE_12))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_CONTROL))
        .cursor_default()
        .bg(bg)
        .text_size(px(TEXT_BASE))
        .text_color(text_color)
        .hover(move |this| this.bg(bg_hover))
        .when(theme.tab_stops, |this| this.tab_index(0))
        .focus(focus_ring(theme))
        .child(label.into());

    button_press(styled, variant, bg, theme)
}

/// A square icon button, as used in the title bar and toolbars.
fn icon_button(id: impl Into<gpui::ElementId>, path: &'static str, theme: &Theme) -> Stateful<Div> {
    let styled = div()
        .id(id.into())
        .w(px(ICON_BUTTON_SIZE))
        .h(px(ICON_BUTTON_SIZE))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_CONTROL))
        .cursor_default()
        .hover(|this| this.bg(theme.element_hover))
        .when(theme.tab_stops, |this| this.tab_index(0))
        .focus(focus_ring(theme))
        .child(icon(path, 14.0, theme.text_muted));

    press_feedback(styled, theme)
}

/// The tooltip view every `.tooltip(..)` call site in this crate builds
/// from: a popover recipe (`surface_overlay`, `RADIUS_CONTROL`,
/// `shadow_popover`, `TEXT_SM`, a hairline `border`).
///
/// gpui 0.2.2 ships no built-in tooltip widget — `.tooltip(..)` only takes a
/// closure that builds an `AnyView`, so *some* `Render` type has to exist to
/// produce one.
struct Tooltip {
    text: SharedString,
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .bg(theme.surface_overlay)
            .border_1()
            .border_color(theme.border)
            .rounded(px(RADIUS_CONTROL))
            .shadow(theme.shadow_popover())
            .px(px(SPACE_8))
            .py(px(SPACE_4))
            .text_size(px(TEXT_SM))
            .text_color(theme.text)
            // gpui prepaints tooltips via `Window::draw_roots`'s
            // `self.prepaint_tooltip(cx)` -> `AnyElement::layout_as_root`
            // (`gpui-0.2.2/src/window.rs`), which runs *after*, and wholly
            // independent of, the main tree's `prepaint_as_root`/deferred
            // passes — it carries no snapshot of `Window::text_style_stack`
            // at all. `app::WtmApp::render`'s root-level
            // `.font_family(theme.font_sans)` therefore cannot reach here;
            // this is the one call site that has to set it itself.
            .font_family(theme.font_sans)
            .child(self.text.clone())
    }
}

/// Build a `.tooltip(..)` closure around [`Tooltip`] — exactly the shape
/// gpui's `StatefulInteractiveElement::tooltip` wants
/// (`impl Fn(&mut Window, &mut App) -> AnyView + 'static`, verified against
/// the vendored `gpui-0.2.2/src/elements/div.rs`). Any `Stateful<Div>`
/// anywhere in the crate can call `.tooltip(ui::tooltip("some text"))`
/// directly now — [`icon_button_with_tooltip`] below is just the first call
/// site, kept as its own helper because it also owns the icon button itself.
pub fn tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_window, cx| cx.new(|_cx| Tooltip { text: text.clone() }).into()
}

/// A square icon button with a named-and-shortcut tooltip, for a titlebar or
/// toolbar control that has no other visible label.
pub fn icon_button_with_tooltip(
    id: impl Into<gpui::ElementId>,
    path: &'static str,
    tooltip_text: &'static str,
    theme: &Theme,
) -> Stateful<Div> {
    icon_button(id, path, theme).tooltip(tooltip(tooltip_text))
}

/// An icon-plus-label toolbar action (e.g. "New Worktree" / "Prune"). Takes
/// a [`ButtonVariant`] because a filled primary ("New Worktree") and a
/// neutral secondary ("Prune") toolbar action both build from this same
/// icon+label shape; a single fixed treatment could not serve both.
pub fn toolbar_button(
    id: impl Into<gpui::ElementId>,
    icon_path: &'static str,
    label: impl Into<SharedString>,
    variant: ButtonVariant,
    theme: &Theme,
) -> Stateful<Div> {
    let (bg, bg_hover, text_color) = button_colors(variant, theme);
    let styled = div()
        .id(id.into())
        .h(px(28.0))
        .px(px(SPACE_12))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(SPACE_6))
        .rounded(px(RADIUS_CONTROL))
        .cursor_default()
        .bg(bg)
        .text_size(px(TEXT_BASE))
        .text_color(text_color)
        .hover(move |this| this.bg(bg_hover))
        .when(theme.tab_stops, |this| this.tab_index(0))
        .focus(focus_ring(theme))
        .child(icon(icon_path, 14.0, text_color))
        .child(label.into());

    button_press(styled, variant, bg, theme)
}

/// A segmented control with a real per-segment click, e.g. the worktree
/// list's sort order (`SortMode`) or the settings sheet's appearance picker
/// (`prefs::Appearance`).
///
/// Every real call site needs a `.on_click(..)` per segment, which is why
/// this — unlike every other component in this file (rule 1 above: "never
/// take a click handler as a parameter") — takes `on_select` and wires one
/// `.on_click(..)` per segment internally instead of returning something the
/// caller could wire up itself: a composite multi-target control has no
/// clean way to hand back N separately-addressable elements from a single
/// `impl IntoElement` return type.
///
/// Generic over `T` (the option's value, e.g. `SortMode`) rather than baking
/// in `&str`/index-based selection, so a caller compares real domain values
/// (`selected == &SortMode::Recent`) instead of juggling a parallel `bool`
/// per option. `on_select` is wrapped in an [`Rc`] (not [`std::sync::Arc`] —
/// gpui element trees are single-threaded, and every other closure captured
/// in this file is a plain `move` closure with no synchronization) so the
/// same callback can be cloned into each segment's own `.on_click(..)`,
/// matching gpui's per-`Stateful<Div>` handler shape (`impl Fn(&ClickEvent,
/// &mut Window, &mut App) + 'static`, verified against the vendored
/// `gpui-0.2.2/src/elements/div.rs`) without requiring `on_select` itself to
/// be `Clone`.
pub fn segmented<T: Clone + PartialEq + 'static>(
    id: impl Into<gpui::ElementId>,
    options: &[(T, &str)],
    selected: &T,
    theme: &Theme,
    on_select: impl Fn(&T, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let on_select = Rc::new(on_select);

    div()
        .id(id.into())
        .flex()
        .flex_none()
        .items_center()
        .p(px(SPACE_2))
        .gap(px(SPACE_2))
        .rounded(px(RADIUS_CONTROL))
        .bg(theme.surface_inset)
        // Its own local tab-index namespace (gpui-0.2.2's `tab_group()`,
        // `elements/div.rs`), so each segment's `tab_index(0..options.len())`
        // below orders the segments relative to *each other* only, and the
        // whole control still occupies a single slot (`tab_index(0)`, the
        // default `tab_group` gives itself) among its siblings — the same
        // convention every other component in this file uses.
        .tab_group()
        .children(options.iter().enumerate().map(|(index, (value, label))| {
            let is_selected = value == selected;
            let value = value.clone();
            let on_select = on_select.clone();
            div()
                .id(index)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .h(px(ROW_HEIGHT - SPACE_2 * 2.0))
                .px(px(SPACE_8))
                .rounded(px(RADIUS_CHIP))
                .cursor_default()
                .text_size(px(TEXT_SM))
                .when(is_selected, |this| {
                    this.bg(theme.surface_raised).text_color(theme.text)
                })
                .when(!is_selected, |this| {
                    this.text_color(theme.text_muted)
                        .hover(|s| s.bg(theme.element_hover))
                })
                .when(theme.tab_stops, |this| this.tab_index(index as isize))
                .focus(focus_ring(theme))
                .child(label.to_string())
                .on_click(move |_event, window, cx| on_select(&value, window, cx))
        }))
}

// ---------------------------------------------------------------------------
// Surfaces
// ---------------------------------------------------------------------------

/// Menu / palette / context-menu surface: the highest plane, `RADIUS_PANEL`,
/// `shadow_popover`. Callers wrap this with `motion::menu_in` at the render
/// call site for the entrance animation — this function only owns the look.
pub fn popover(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .bg(theme.surface_overlay)
        .border_1()
        .border_color(theme.border_strong)
        .rounded(px(RADIUS_PANEL))
        .shadow(theme.shadow_popover())
        // `.occlude()`, not `.block_mouse_except_scroll()`: this panel sits
        // over a scrollable surface (the worktree list, a dialog card) and
        // must swallow the scroll wheel too, or scrolling inside a popover
        // (the palette's results, a context menu, the base-ref picker)
        // also scrolls whatever is stacked behind it. gpui's own doc on
        // `block_mouse_except_scroll` says to prefer it in general — that
        // advice does not apply here, because blocking scroll from
        // reaching what's behind is exactly the point. Do not "simplify"
        // this back to `block_mouse_except_scroll`.
        .occlude()
}

/// A full-window scrim behind a modal, centering whatever dialog sits on
/// top of it. The caller stacks this as a top-level, absolutely-positioned
/// child — it is not meant to participate in normal layout flow.
///
/// `scrim` is context-free, so this needs no `&Theme`.
pub fn modal_backdrop() -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(scrim(SCRIM_ALPHA_DARK))
        // Covers the whole window, so `.occlude()` here is what stops a
        // scroll wheel over the backdrop (or over the card sitting on top
        // of it) from reaching the worktree list painted underneath — see
        // `popover`'s doc for why `.occlude()`, not
        // `block_mouse_except_scroll`, is correct for a modal surface.
        .occlude()
}

/// The dialog surface itself: a raised panel with enough shadow to read as
/// floating above the scrim, at a fixed width so its content doesn't
/// reflow while its data loads. `RADIUS_DIALOG`; fields inside it use
/// `RADIUS_CONTROL`.
pub fn modal_card(width: f32, theme: &Theme) -> Div {
    div()
        .w(px(width))
        .flex()
        .flex_col()
        .bg(theme.surface_dialog)
        .border_1()
        .border_color(theme.border_strong)
        .rounded(px(RADIUS_DIALOG))
        .shadow(theme.shadow_dialog())
        // See `popover`'s doc: `.occlude()` (not `block_mouse_except_scroll`)
        // stops a scroll wheel over the card — including over any part of
        // it that isn't itself a scroll region — from reaching the
        // worktree list behind the backdrop.
        .occlude()
        // Its own tab-index namespace (gpui-0.2.2's `tab_group()`), so Tab
        // inside an open dialog cycles the card's own fields/toggles/footer
        // buttons in paint order without interleaving with whatever's
        // painted at the same nesting depth elsewhere in the tree. This is
        // ordering only, not containment — see `Theme::tab_stops`'s doc for
        // how the background shell is actually kept out of the tab order
        // while a dialog covers it.
        .tab_group()
}

/// A modal's title, with an optional supporting line beneath it.
pub fn modal_header(
    title: impl Into<SharedString>,
    subtitle: Option<&str>,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_2))
        .px(px(SPACE_16))
        .pt(px(SPACE_16))
        .child(
            div()
                .text_size(px(TEXT_LG))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(title.into()),
        )
        .when_some(subtitle.map(str::to_string), |this, subtitle| {
            this.child(
                div()
                    .text_size(px(TEXT_SM))
                    .text_color(theme.text_muted)
                    .child(subtitle),
            )
        })
}

/// A right-aligned button row along a modal's bottom edge, separated from
/// the body by a hairline so it reads as the dialog's fixed action bar even
/// when the content above it scrolls — structural, unlike most in-pane
/// separators, which are spacing instead.
pub fn modal_footer(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_end()
        .gap(px(SPACE_8))
        .px(px(SPACE_16))
        .py(px(SPACE_12))
        .border_t_1()
        .border_color(theme.border)
        // Its own tab-index namespace, same reasoning as `modal_card`'s own
        // `.tab_group()` — keeps Cancel/Create (or Cancel/Remove, etc.)
        // ordered as one self-contained unit within the dialog's tab order.
        .tab_group()
}

// ---------------------------------------------------------------------------
// States
// ---------------------------------------------------------------------------

/// Centered, muted helper text for an empty state inside a panel — the
/// "Run `wtm` inside a repository…" line's shape, generalized so other
/// panels don't have to hand-roll it. A designed empty state (icon,
/// headline, hint) should prefer [`empty_state`] instead.
pub fn empty_hint(text: impl Into<SharedString>, theme: &Theme) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(TEXT_SM))
        .text_color(theme.text_muted)
        .child(text.into())
}

/// [`empty_hint`]'s error sibling: the same centered, panel-filling layout,
/// but in `theme.danger` with a leading alert icon so an error reads as
/// distinct from "loading…"/"nothing here" at a glance rather than only in
/// its wording — color is never the only channel, hence the icon, same rule
/// [`inline_error`] follows for a dialog field.
pub fn empty_hint_error(message: impl Into<SharedString>, theme: &Theme) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(SPACE_6))
        .text_size(px(TEXT_SM))
        .text_color(theme.danger)
        .child(icon(crate::assets::icons::CIRCLE_ALERT, 12.0, theme.danger))
        .child(div().min_w_0().child(message.into()))
}

/// A designed empty state: icon above headline, a supporting hint line, and
/// an optional action slot (e.g. "No worktrees yet" → `New Worktree`) — an
/// empty state is a designed state, not an absence of one.
pub fn empty_state(
    icon_path: &'static str,
    title: impl Into<SharedString>,
    hint: impl Into<SharedString>,
    action: Option<AnyElement>,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(SPACE_8))
        .px(px(SPACE_32))
        .child(icon(icon_path, 28.0, theme.text_faint))
        .child(
            div()
                .text_size(px(TEXT_XL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(title.into()),
        )
        .child(
            div()
                .text_size(px(TEXT_BASE))
                .text_color(theme.text_muted)
                .child(hint.into()),
        )
        .when_some(action, |this, action| {
            this.child(div().pt(px(SPACE_8)).child(action))
        })
}

/// A loading placeholder: a static, rounded `surface_inset` block. Preferred
/// over a spinner for recent-commits loading — the shape of what is coming
/// is better feedback than a spinner — so this deliberately does not
/// animate.
pub fn skeleton(width: f32, height: f32, theme: &Theme) -> Div {
    div()
        .w(px(width))
        .h(px(height))
        .flex_none()
        .rounded(px(RADIUS_CHIP))
        .bg(theme.surface_inset)
}

/// An inline validation error: icon + message in `danger`, for under a
/// dialog field. Color is never the only channel, hence the icon.
pub fn inline_error(message: impl Into<SharedString>, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(SPACE_6))
        .text_size(px(TEXT_SM))
        .text_color(theme.danger)
        .child(icon(crate::assets::icons::CIRCLE_ALERT, 12.0, theme.danger))
        .child(div().min_w_0().child(message.into()))
}

// ---------------------------------------------------------------------------
// Scroll affordance
// ---------------------------------------------------------------------------

/// A gradient that fades content into `surface` at a scroll region's top
/// edge. `linear_gradient` over an **opaque** surface only — over the
/// translucent (vibrancy) sidebar there is nothing to fade into, because
/// "what is behind the window" is not a paintable color.
/// Used on the worktree list and the detail panel's Details/Changes tabs
/// (`app::chrome`); never on the sidebar (translucent) or the Files tab's
/// tree column (`SPACE_2`-wide, no room to spare — see that call site).
/// Caller decides *whether* to show it via [`scroll_edges`] — this function
/// only paints.
pub fn scroll_fade_top(surface: Hsla, height: f32) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(px(height))
        .flex_none()
        .bg(linear_gradient(
            180.0,
            linear_color_stop(surface, 0.0),
            linear_color_stop(surface.opacity(0.0), 1.0),
        ))
}

/// The bottom-edge counterpart to [`scroll_fade_top`]. Same call sites.
pub fn scroll_fade_bottom(surface: Hsla, height: f32) -> Div {
    div()
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .h(px(height))
        .flex_none()
        .bg(linear_gradient(
            0.0,
            linear_color_stop(surface, 0.0),
            linear_color_stop(surface.opacity(0.0), 1.0),
        ))
}

/// Which edges of a scroll region currently have hidden content to fade
/// into — the pure predicate behind "show [`scroll_fade_top`]/
/// [`scroll_fade_bottom`] when there is overflow on that edge, hide it when
/// there is not." Kept separate from painting so it is unit-testable
/// without a live `ScrollHandle`/window.
///
/// `offset`/`max_offset` use gpui's own `ScrollHandle` sign convention
/// (`ScrollHandle::offset()`/`max_offset()`, `elements/div.rs`): `offset` is
/// `<= 0` and grows more negative as the region scrolls down/right;
/// `max_offset` is the total overflow, always `>= 0` (`0` means nothing to
/// scroll at all). [`scrollbar_thumb`] below takes the same convention, for
/// the same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollEdges {
    /// There is hidden content above/before the visible region.
    pub leading: bool,
    /// There is hidden content below/after the visible region.
    pub trailing: bool,
}

pub fn scroll_edges(offset: f32, max_offset: f32) -> ScrollEdges {
    if max_offset <= 0.0 {
        return ScrollEdges::default();
    }
    ScrollEdges {
        leading: offset < 0.0,
        trailing: offset > -max_offset,
    }
}

// ---------------------------------------------------------------------------
// Scrollbar
// ---------------------------------------------------------------------------

/// gpui 0.2.2 ships no scrollbar widget at all, so this is a hand-built one:
/// `ScrollHandle::offset()`/`max_offset()` for geometry, and gpui's
/// drag-and-drop pair `on_drag`/`on_drag_move`, repurposed for a non-DnD
/// drag the way `on_drag_move`'s own doc invites: "useful for implementing
/// draggable UIs that don't conform to a drag and drop style interaction,
/// like resizing."
///
/// # Why drag-and-drop primitives for a scrollbar thumb
///
/// A plain `.on_mouse_move(..)` only fires while the pointer stays over the
/// element's own hitbox — exactly wrong for a drag, since a fast drag
/// routinely carries the pointer outside a several-pixel-wide thumb.
/// `on_drag_move::<T>` is the one listener gpui dispatches regardless of
/// hover, for as long as a same-typed drag is active, which is what keeps
/// the thumb tracking the cursor past its own edges. The trade-off: gpui
/// matches `on_drag_move::<T>` purely by `TypeId`, not by which element
/// started the drag, so *every* mounted scrollbar receives every move event
/// once any one of them is being dragged — see [`ScrollbarDrag`]'s `id`
/// field for how each listener ignores a drag that isn't its own.
///
/// # Wiring
///
/// The caller wraps its scroll container in its own `.relative()` ancestor
/// and adds this element as a **sibling** of the scrolling div, never a
/// descendant — a child of the scrolling element scrolls away with its own
/// content, which would drag the overlay out of view along with the list.
///
/// Renders an empty, zero-size element when [`scrollbar_thumb`] finds no
/// overflow, so it never reserves layout room.
pub fn scrollbar(id: impl Into<ElementId>, handle: &ScrollHandle, _axis: ScrollAxis) -> AnyElement {
    let id = id.into();
    let bounds = handle.bounds();
    let max = handle.max_offset();
    let off = handle.offset();
    let viewport = f32::from(bounds.size.height);
    let max_offset = f32::from(max.height);
    let offset = f32::from(off.y);

    let Some(geometry) = scrollbar_thumb(viewport, max_offset, offset) else {
        return div().into_any_element();
    };

    let track_id = id.clone();
    let thumb_color = hairline(0.5);
    let thumb_hover_color = hairline(0.9);

    let thumb = div()
        .id(id)
        .absolute()
        .rounded(px(RADIUS_CHIP))
        .bg(thumb_color)
        .hover(|s| s.bg(thumb_hover_color))
        .on_drag(
            ScrollbarDrag {
                id: track_id.clone(),
                handle: handle.clone(),
            },
            |_payload, _cursor_offset, _window, cx| cx.new(|_cx| DragGhost),
        )
        .top(px(geometry.position))
        .right(px(SCROLLBAR_INSET))
        .w(px(SCROLLBAR_THICKNESS))
        .h(px(geometry.length));

    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        // Deliberately *not* `.id(..)` — `on_drag_move` is an
        // `InteractiveElement`-only method (unlike `on_drag`, it needs no
        // `Stateful<Div>`), and this track div's only job is supplying
        // `DragMoveEvent::bounds` (its own painted bounds, used below to
        // convert the live cursor position back into a scroll fraction).
        .on_drag_move::<ScrollbarDrag>(move |event, _window, cx| {
            let drag = event.drag(cx);
            if drag.id != track_id {
                // Not this scrollbar's own drag — see `ScrollbarDrag::id`'s
                // doc for why every mounted scrollbar sees every drag move
                // event and must filter for its own.
                return;
            }
            let track_bounds = event.bounds;
            let track_len = f32::from(track_bounds.size.height);
            let track_origin = f32::from(track_bounds.origin.y);
            let cursor = f32::from(event.event.position.y);
            let max_offset = f32::from(drag.handle.max_offset().height);
            let Some(new_offset) =
                scrollbar_drag_offset(track_len, track_origin, cursor, max_offset)
            else {
                return;
            };
            let current = drag.handle.offset();
            drag.handle.set_offset(point(current.x, px(new_offset)));
        })
        .child(thumb)
        .into_any_element()
}

/// Axis a [`scrollbar`] tracks. Every scroll region in this app is vertical
/// today; kept as a parameter (rather than dropped) so a future
/// horizontally-scrolling region doesn't need every call site updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollAxis {
    Vertical,
}

/// Scrollbar thumb thickness — thin and quiet at rest, derived from
/// [`SPACE_6`] rather than an invented pixel value.
const SCROLLBAR_THICKNESS: f32 = SPACE_6;
/// Gap between the thumb and the scroll region's own edge.
const SCROLLBAR_INSET: f32 = SPACE_2;
/// Floor on thumb length so a very long list's thumb never shrinks below a
/// comfortably grabbable size — [`SPACE_32`], the scale this module already
/// uses elsewhere for minimum interactive sizes.
const SCROLLBAR_MIN_THUMB: f32 = SPACE_32;

/// One scrollbar thumb's length and position within its track, in px along
/// the scroll axis — pure arithmetic (tested below) so painting and the
/// "is there overflow at all" check can never disagree.
///
/// `viewport`/`max_offset`/`offset` all read straight off a live
/// `ScrollHandle` — see [`scroll_edges`]'s doc for the shared sign
/// convention. `None` when there is nothing to scroll (`max_offset <= 0`, or
/// a degenerate `viewport <= 0` before the region's first real layout
/// pass) — the caller hides the scrollbar entirely in that case: show it
/// when there is overflow, hide it when there is not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThumbGeometry {
    /// Thumb length along the scroll axis, in px.
    pub length: f32,
    /// Distance from the track's leading edge to the thumb's leading edge.
    pub position: f32,
}

pub fn scrollbar_thumb(viewport: f32, max_offset: f32, offset: f32) -> Option<ThumbGeometry> {
    if viewport <= 0.0 || max_offset <= 0.0 {
        return None;
    }
    let content = viewport + max_offset;
    let min_thumb = SCROLLBAR_MIN_THUMB.min(viewport);
    let length = (viewport * viewport / content).clamp(min_thumb, viewport);
    let travel = (viewport - length).max(0.0);
    let scrolled = (-offset).clamp(0.0, max_offset);
    let fraction = scrolled / max_offset;
    Some(ThumbGeometry {
        length,
        position: travel * fraction,
    })
}

/// The drag half of [`scrollbar_thumb`]: given the track's own painted
/// length/origin (`DragMoveEvent::bounds`), the live cursor position, and
/// the handle's `max_offset`, returns gpui's own signed scroll offset
/// (`<= 0`, see [`scroll_edges`]'s doc) that puts the thumb's *center*
/// under the cursor.
///
/// Centering under the cursor rather than preserving the exact pixel the
/// user grabbed is a deliberate simplification: gpui 0.2.2's
/// `on_drag`/`on_drag_move` pair carries a drag *payload* fixed at the
/// moment the drag element was last built (before the mouse-down that
/// starts the drag even happens — verified against `elements/div.rs`'s
/// `on_drag`/`on_drag_move`), with no channel back from "where inside the
/// thumb did the mouse go down" into that payload. Recomputing the offset
/// fresh from the current cursor position every event sidesteps needing
/// that channel at all, at the cost of a small jump on the very first move
/// past the drag threshold when the grab point wasn't already the thumb's
/// center — an acceptable trade for a scrollbar thumb, where every position
/// within the thumb represents the same "jump to here" intent anyway.
///
/// `None` when the track has no travel to give (`max_offset <= 0`, a
/// degenerate `track_len <= 0`, or a thumb that already fills the whole
/// track) — the caller leaves the handle's offset untouched rather than
/// dividing by zero.
pub fn scrollbar_drag_offset(
    track_len: f32,
    track_origin: f32,
    cursor: f32,
    max_offset: f32,
) -> Option<f32> {
    if track_len <= 0.0 || max_offset <= 0.0 {
        return None;
    }
    let content = track_len + max_offset;
    let min_thumb = SCROLLBAR_MIN_THUMB.min(track_len);
    let thumb_len = (track_len * track_len / content).clamp(min_thumb, track_len);
    let travel = track_len - thumb_len;
    if travel <= 0.0 {
        return None;
    }
    let target = (cursor - track_origin - thumb_len / 2.0).clamp(0.0, travel);
    let fraction = target / travel;
    Some(-(fraction * max_offset))
}

/// Drag payload carried while a [`scrollbar`] thumb is being dragged — see
/// [`scrollbar`]'s own "why drag-and-drop primitives" doc for the mechanism
/// this rides on. `id` exists solely so two independently-mounted
/// scrollbars sharing an axis (e.g. the Files tab's tree column and its
/// diff both scroll vertically at once) never cross-talk: gpui dispatches
/// `on_drag_move::<ScrollbarDrag>` to *every* mounted listener of this type
/// once any one of them starts a drag, matching purely by `TypeId` and not
/// by which element originated it, so every listener must check `id`
/// against its own before touching `handle`.
#[derive(Clone)]
struct ScrollbarDrag {
    id: ElementId,
    handle: ScrollHandle,
}

/// The `on_drag` API requires *some* `Render` view to show under the cursor
/// while dragging — ordinary drag-and-drop uses this to preview the thing
/// being dropped. A scrollbar thumb drag has no such payload to preview
/// (the thumb itself stays put, tracking the scroll position — it is not
/// "the thing being moved" in the DnD sense), so this renders nothing; it
/// exists only because `on_drag`'s `constructor` parameter is required to
/// return *some* `Entity<impl Render>`.
struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;

    #[test]
    fn scroll_edges_hides_both_when_there_is_nothing_to_scroll() {
        let edges = scroll_edges(0.0, 0.0);
        assert!(!edges.leading);
        assert!(!edges.trailing);
    }

    #[test]
    fn scroll_edges_shows_only_trailing_at_the_very_top() {
        let edges = scroll_edges(0.0, 200.0);
        assert!(!edges.leading, "nothing scrolled past yet");
        assert!(edges.trailing, "more content below");
    }

    #[test]
    fn scroll_edges_shows_only_leading_at_the_very_bottom() {
        let edges = scroll_edges(-200.0, 200.0);
        assert!(edges.leading, "content scrolled past above");
        assert!(!edges.trailing, "nothing more below");
    }

    #[test]
    fn scroll_edges_shows_both_in_the_middle() {
        let edges = scroll_edges(-100.0, 200.0);
        assert!(edges.leading);
        assert!(edges.trailing);
    }

    #[test]
    fn scrollbar_thumb_hides_when_content_fits_the_viewport() {
        assert_eq!(scrollbar_thumb(400.0, 0.0, 0.0), None);
    }

    #[test]
    fn scrollbar_thumb_hides_on_a_degenerate_zero_viewport() {
        // The very first frame, before the tracked div's own bounds have
        // ever been painted — `ScrollHandle::bounds()` starts at
        // `Bounds::default()`. Must not divide by zero.
        assert_eq!(scrollbar_thumb(0.0, 500.0, 0.0), None);
    }

    #[test]
    fn scrollbar_thumb_sizes_proportionally_to_the_visible_fraction() {
        // 400px viewport over 800px content (half visible) => thumb is half
        // the track, comfortably above the min-thumb floor.
        let geometry = scrollbar_thumb(400.0, 400.0, 0.0).expect("has overflow");
        assert!((geometry.length - 200.0).abs() < 1.0);
        assert_eq!(geometry.position, 0.0, "at the top, thumb starts flush");
    }

    #[test]
    fn scrollbar_thumb_never_shrinks_below_the_min_size_on_a_huge_list() {
        // 400px viewport over 40,000px content — the proportional formula
        // alone would compute a ~4px thumb, unusably small to grab.
        let geometry = scrollbar_thumb(400.0, 39_600.0, 0.0).expect("has overflow");
        assert_eq!(geometry.length, SCROLLBAR_MIN_THUMB);
    }

    #[test]
    fn scrollbar_thumb_reaches_the_track_end_when_fully_scrolled() {
        let max_offset = 400.0;
        let geometry = scrollbar_thumb(400.0, max_offset, -max_offset).expect("has overflow");
        let travel = 400.0 - geometry.length;
        assert!((geometry.position - travel).abs() < 1e-3);
    }

    #[test]
    fn scrollbar_thumb_position_is_monotonic_in_scroll_offset() {
        let max_offset = 1000.0;
        let mut last = -1.0;
        let mut offset = 0.0;
        while offset >= -max_offset {
            let geometry = scrollbar_thumb(300.0, max_offset, offset).unwrap();
            assert!(geometry.position >= last - 1e-6);
            last = geometry.position;
            offset -= 100.0;
        }
    }

    #[test]
    fn scrollbar_drag_offset_centers_the_thumb_under_the_cursor() {
        // 400px track, 400px overflow => thumb length is
        // `400*400/800 = 200px`, so `travel = 200px`. Cursor at the track's
        // exact midpoint (200px from `track_origin`) should center the
        // 200px thumb there too: `target = 200 - 100 = 100`, half of
        // `travel` => half of `max_offset` scrolled.
        let offset = scrollbar_drag_offset(400.0, 0.0, 200.0, 400.0).expect("has travel to give");
        assert!((offset - (-200.0)).abs() < 1.0);
    }

    #[test]
    fn scrollbar_drag_offset_clamps_past_either_end_of_the_track() {
        let past_start = scrollbar_drag_offset(400.0, 0.0, -1000.0, 400.0).unwrap();
        assert_eq!(past_start, 0.0);
        let past_end = scrollbar_drag_offset(400.0, 0.0, 1000.0, 400.0).unwrap();
        assert_eq!(past_end, -400.0);
    }

    #[test]
    fn scrollbar_drag_offset_is_none_when_there_is_no_travel() {
        assert_eq!(scrollbar_drag_offset(0.0, 0.0, 0.0, 400.0), None);
        assert_eq!(scrollbar_drag_offset(400.0, 0.0, 0.0, 0.0), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shade_clamps_lightness_to_the_unit_range() {
        let base = Hsla {
            h: 0.5,
            s: 0.5,
            l: 0.05,
            a: 1.0,
        };
        assert_eq!(shade(base, -0.5).l, 0.0, "cannot go below 0");
        let bright = Hsla {
            h: 0.5,
            s: 0.5,
            l: 0.95,
            a: 1.0,
        };
        assert_eq!(shade(bright, 0.5).l, 1.0, "cannot exceed 1");
        assert!((shade(base, 0.1).l - 0.15).abs() < 1e-6);
    }
}

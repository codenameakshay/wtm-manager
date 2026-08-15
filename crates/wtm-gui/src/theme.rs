//! The app's visual language.
//!
//! Neutral graphite surfaces where color carries no meaning, so the colors
//! that *do* mean something — dirty, ahead, behind, gone — read instantly in a
//! list. Surfaces are layered by elevation (`inset` < `canvas` < `raised`)
//! rather than by hue, and every interactive row shares one 6% neutral wash
//! for hover, selection, and press instead of inventing a tint per state.
//!
//! The accent is wtm's own: the orange from the app icon's gradient. It is
//! used for identity and focus only, never for structure.
//!
//! The palette is resolved once at startup from the system appearance and
//! published as a gpui global, which is how every view reads it.

use gpui::{hsla, rgb, App, Global, Hsla, WindowAppearance};

#[derive(Clone, Copy)]
pub struct Theme {
    /// The window's base surface.
    pub canvas: Hsla,
    /// The sidebar, painted *over* the window's blurred backing.
    ///
    /// Deliberately not fully transparent: raw vibrancy takes its tone from
    /// whatever wallpaper happens to be behind the window, which turns a dark
    /// sidebar light — and its text unreadable — the moment someone uses a
    /// light desktop picture. A mostly-opaque tint keeps the blur's depth
    /// while the theme keeps control of contrast.
    pub sidebar: Hsla,
    /// Panels that sit above the canvas (headers, footers).
    pub raised: Hsla,
    /// Recessed areas (list backgrounds, inputs).
    pub inset: Hsla,
    /// The one neutral wash used for hover, selection, and press.
    pub item_wash: Hsla,
    /// A slightly stronger wash for the selected row, so selection still
    /// reads when the pointer is elsewhere.
    pub item_selected: Hsla,

    pub border: Hsla,
    pub border_strong: Hsla,

    pub text: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub text_ghost: Hsla,

    /// wtm's brand orange. Identity and focus rings only.
    pub accent: Hsla,

    pub success: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
    pub info: Hsla,
}

impl Theme {
    /// The palette in effect, falling back to dark before `init` has run.
    pub fn of(cx: &App) -> Self {
        if cx.has_global::<ActiveTheme>() {
            cx.global::<ActiveTheme>().0
        } else {
            Self::dark()
        }
    }

    pub fn dark() -> Self {
        Self {
            canvas: rgb(0x1A1A1A).into(),
            sidebar: hsla(0.0, 0.0, 0.094, 0.82),
            raised: rgb(0x212121).into(),
            inset: rgb(0x151515).into(),
            item_wash: hsla(0.0, 0.0, 0.941, 0.06),
            item_selected: hsla(0.0, 0.0, 0.941, 0.10),

            border: hsla(220.0 / 360.0, 0.10, 0.90, 0.07),
            border_strong: hsla(220.0 / 360.0, 0.10, 0.90, 0.14),

            text: rgb(0xE2E2E2).into(),
            text_secondary: rgb(0xA3A3A3).into(),
            text_tertiary: rgb(0x7D7D7D).into(),
            text_ghost: rgb(0x575757).into(),

            // The app icon's gradient runs #F97316 → #EF4444; the accent is
            // the orange end, eased back slightly so it sits calmly on a dark
            // surface instead of vibrating against it.
            accent: rgb(0xF08A4B).into(),

            success: rgb(0x62C987).into(),
            warning: rgb(0xE0B36A).into(),
            danger: rgb(0xE2726A).into(),
            info: rgb(0x7FA9E8).into(),
        }
    }

    pub fn light() -> Self {
        Self {
            canvas: rgb(0xF6F5F6).into(),
            sidebar: hsla(0.0, 0.0, 0.953, 0.82),
            raised: rgb(0xFFFFFF).into(),
            inset: rgb(0xECECEC).into(),
            item_wash: hsla(0.0, 0.0, 0.078, 0.06),
            item_selected: hsla(0.0, 0.0, 0.078, 0.10),

            border: hsla(220.0 / 360.0, 0.10, 0.12, 0.08),
            border_strong: hsla(220.0 / 360.0, 0.10, 0.12, 0.15),

            text: rgb(0x242424).into(),
            text_secondary: rgb(0x666666).into(),
            text_tertiary: rgb(0x858585).into(),
            text_ghost: rgb(0xA4A4A4).into(),

            accent: rgb(0xD2620E).into(),

            success: rgb(0x2F8F52).into(),
            warning: rgb(0xA66B20).into(),
            danger: rgb(0xC64A42).into(),
            info: rgb(0x3F6FBF).into(),
        }
    }
}

#[derive(Clone, Copy)]
struct ActiveTheme(Theme);

impl Global for ActiveTheme {}

/// Resolve the palette from the system appearance and publish it. Called
/// before any window exists.
pub fn init(cx: &mut App) {
    let theme = for_appearance(cx.window_appearance());
    cx.set_global(ActiveTheme(theme));
}

/// Re-resolve after the system switches between light and dark.
pub fn refresh(appearance: WindowAppearance, cx: &mut App) {
    cx.set_global(ActiveTheme(for_appearance(appearance)));
    cx.refresh_windows();
}

fn for_appearance(appearance: WindowAppearance) -> Theme {
    match appearance {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Theme::dark(),
        WindowAppearance::Light | WindowAppearance::VibrantLight => Theme::light(),
    }
}

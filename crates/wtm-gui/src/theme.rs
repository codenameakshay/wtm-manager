//! The app's visual language.
//!
//! Colors are authored in **OKLCH** (Björn Ottosson's perceptually-even color
//! space — the same one CSS Color 4 and Tailwind v4 use) and converted to
//! gpui [`Hsla`] once, at palette-construction time. The conversion math
//! (`oklch_to_srgb`, `rgb_to_hsl`, `hsl_to_rgb`, `gamma_encode`) plus the
//! contrast helpers (`relative_luminance`, `contrast_ratio`, `flatten`) are
//! ported from Zeron/comet's `crates/ui/src/theme.rs`, which is itself
//! test-anchored against independently-computed anchors.
//!
//! # Numbers drive layout, colors are paint
//!
//! Layout constants (radii, spacing, the density table) are plain `f32`
//! consts that never depend on which appearance is painted — there is
//! deliberately no `if dark { px(8) } else { px(10) }` anywhere in this
//! module.
//!
//! # Light is designed, not inverted
//!
//! 1. **Surface order flips.** In dark, the content plane (`bg`) is the
//!    *deepest* plane and chrome (`surface`) sits one step up. In light, the
//!    content plane is pure white and chrome goes *grey* — chrome recedes in
//!    both appearances, which is the direction a naive invert gets backwards.
//! 2. **The card/dialog/overlay elevation ladder can't reuse the dark trick.**
//!    Dark distinguishes floating planes by small lightness steps. Light has
//!    nothing lighter than white to climb to, so all three land on white and
//!    let `border` + shadow carry the separation instead.
//! 3. **Accents move down the scale.** wtm's orange stays orange in both
//!    appearances, but light mode needs a darker, more saturated step to
//!    clear WCAG AA on a white field — see [`Theme::light`]'s accent value.
//!
//! # The paint helpers
//!
//! [`hairline`] and [`scrim`] are **free functions**, not `Theme` methods,
//! because they are called from element builders deep in the tree that have
//! no `cx` in scope. They read a process-wide appearance mirror
//! ([`CURRENT_APPEARANCE`]) instead — [`Theme::install`] (via [`init`] and
//! [`refresh`]) is the only writer outside tests. Alphas passed to these
//! helpers are always quoted in **dark-mode terms**: the dark palette is the
//! tuned one, light derives from it via [`INK_FILL_SCALE`] / [`INK_HAIRLINE_SCALE`].
//!
//! Installed as a gpui [`Global`] at boot ([`init`]); read with [`Theme::of`].

use std::sync::atomic::{AtomicU8, Ordering};

use gpui::{hsla, point, px, App, BoxShadow, Global, Hsla, WindowAppearance};

// ---------------------------------------------------------------------------
// Appearance + the process-wide mirror
// ---------------------------------------------------------------------------

/// Which appearance is currently painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Appearance {
    #[default]
    Dark,
    Light,
}

impl Appearance {
    /// Map a gpui window appearance onto ours — the vibrant variants are just
    /// the blurred flavor of the same tone.
    pub fn from_window(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::Dark,
        }
    }
}

/// Process-wide mirror of the installed theme's appearance.
///
/// [`hairline`] and [`scrim`] are free functions called from element
/// builders with no `cx` in scope, so they read the appearance from here
/// rather than from the gpui global. Appearance is genuinely process-wide
/// (one setting for every window), so a single mirror is sound.
static CURRENT_APPEARANCE: AtomicU8 = AtomicU8::new(0);

fn current_appearance() -> Appearance {
    match CURRENT_APPEARANCE.load(Ordering::Relaxed) {
        1 => Appearance::Light,
        _ => Appearance::Dark,
    }
}

/// Point the context-free paint helpers at an appearance. The **only**
/// writer of [`CURRENT_APPEARANCE`] outside tests — [`Theme::install`] (and
/// through it, [`init`]/[`refresh`]) is the sole call site.
fn set_current_appearance(appearance: Appearance) {
    let encoded = match appearance {
        Appearance::Dark => 0,
        Appearance::Light => 1,
    };
    CURRENT_APPEARANCE.store(encoded, Ordering::Relaxed);
}

/// [`CURRENT_APPEARANCE`] is process-wide, so under the parallel test runner
/// any test that flips it (or asserts on a helper that reads it) must hold
/// this lock. Tests that flip the appearance restore `Dark` before releasing
/// the guard.
#[cfg(test)]
pub(crate) fn lock_appearance() -> std::sync::MutexGuard<'static, ()> {
    static APPEARANCE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    APPEARANCE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// Paint-helper scales
// ---------------------------------------------------------------------------

/// Light-mode alpha multiplier for **fills** (hover/active washes, chip and
/// pill backgrounds). The same number in both appearances — only the tone
/// flips — because the dark palette's fill alphas are already tuned to the
/// established light-UI scale (subtle ≈ 3-4%, hover ≈ 8%, selected ≈ 14%).
pub const INK_FILL_SCALE: f32 = 1.0;

/// Light-mode alpha multiplier for **hairlines** (borders, dividers, rings).
/// Opposite of fills: a 1px edge has to hold its own against a bright
/// surround, so hairlines scale UP rather than staying flat.
pub const INK_HAIRLINE_SCALE: f32 = 1.35;

/// Dark-mode alpha of the standard modal backdrop. Call sites that need a
/// heavier or lighter scrim pass their own dark-mode alpha to [`scrim`].
pub const SCRIM_ALPHA_DARK: f32 = 0.55;

/// Sidebar tint translucency over the blurred window backing (macOS
/// vibrancy). Opaque elsewhere: Linux/Windows get no compositor-blur
/// guarantee, and a merely transparent window would show raw desktop through
/// the sidebar.
pub const GLASS_ALPHA: f32 = if cfg!(target_os = "macos") { 0.82 } else { 1.0 };
/// Light-mode frost alpha. Runs heavier than dark's: a light tint controls
/// the blur less, so the desktop's color bleeds through more readily.
pub const GLASS_ALPHA_LIGHT: f32 = if cfg!(target_os = "macos") { 0.85 } else { 1.0 };

// ---------------------------------------------------------------------------
// Layout — numbers only, never appearance-dependent
// ---------------------------------------------------------------------------

/// Modal card corner radius.
pub const RADIUS_DIALOG: f32 = 14.0;
/// Panels, popovers, inline cards.
pub const RADIUS_PANEL: f32 = 10.0;
/// List rows, action rows.
pub const RADIUS_ROW: f32 = 8.0;
/// Buttons, inputs, chips.
pub const RADIUS_CONTROL: f32 = 6.0;
/// Kbd caps, tiny pills, nested chips.
pub const RADIUS_CHIP: f32 = 4.0;

/// The spacing scale (px). Per `better-layout`: the gap *between* groups
/// should be at least 2x the gap *within* a group.
pub const SPACE_2: f32 = 2.0;
pub const SPACE_4: f32 = 4.0;
pub const SPACE_6: f32 = 6.0;
pub const SPACE_8: f32 = 8.0;
pub const SPACE_12: f32 = 12.0;
pub const SPACE_16: f32 = 16.0;
pub const SPACE_20: f32 = 20.0;
pub const SPACE_24: f32 = 24.0;
pub const SPACE_32: f32 = 32.0;

/// Height of the window's title bar strip.
pub const TITLEBAR_HEIGHT: f32 = 44.0;
/// Horizontal room the macOS traffic lights need before content may start.
pub const TRAFFIC_LIGHT_CLEARANCE: f32 = 78.0;
/// Single-line action row height.
pub const ROW_HEIGHT: f32 = 32.0;
/// Two-line worktree card height.
pub const LIST_ROW_HEIGHT: f32 = 56.0;
/// The list wraps each row in 2px of bottom padding, so keyboard-scroll math
/// must step by this, not by [`LIST_ROW_HEIGHT`] alone.
pub const LIST_ROW_PITCH: f32 = LIST_ROW_HEIGHT + 2.0;
/// Sidebar width.
pub const SIDEBAR_WIDTH: f32 = 248.0;
/// Footer strip height.
pub const FOOTER_HEIGHT: f32 = 28.0;
/// Square icon-button hit area (titlebar, sidebar, toolbars): a macOS
/// toolbar button reads closer to ~28, and at `TITLEBAR_HEIGHT` (44) this
/// still leaves 8px of clearance above and below. The one source of truth
/// for every hand-built copy of this control (`ui::icon_button`,
/// `app/chrome.rs`'s reload button and its Linux window-control buttons).
pub const ICON_BUTTON_SIZE: f32 = 28.0;

/// The text scale (px). `ui.rs` re-exports every one of these under the same
/// name so its `ui::TEXT_*` call sites keep working.
///
/// Shortcut hints, kbd caps.
pub const TEXT_XS: f32 = 11.0;
/// Meta lines, pills, footer.
pub const TEXT_SM: f32 = 12.0;
/// Row labels, body.
pub const TEXT_BASE: f32 = 13.0;
/// Section titles, dialog field labels; `detail_panel.rs`'s branch name (one
/// step heavier than the list row) is this scale's real call site today.
pub const TEXT_MD: f32 = 14.0;
/// Dialog titles.
pub const TEXT_LG: f32 = 16.0;
/// Empty-state headlines.
pub const TEXT_XL: f32 = 20.0;

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

/// The bundled sans family's name — the family baked into `Geist.ttf` and
/// its static weight cuts (`Geist-Medium.ttf`/`Geist-SemiBold.ttf`/
/// `Geist-Bold.ttf`), registered by `assets::register_fonts`. The single
/// source of truth for [`Theme::font_sans`].
pub const FONT_SANS_DEFAULT: &str = "Geist";
/// The bundled monospace family's name (`GeistMono.ttf`). The single source
/// of truth for [`Theme::font_mono`] *and* `ui::FONT_MONO`, which re-exports
/// this constant instead of repeating the literal independently.
pub const FONT_MONO_DEFAULT: &str = "Geist Mono";

// ---------------------------------------------------------------------------
// OKLCH primitives
// ---------------------------------------------------------------------------

/// Convert an oklch color (CSS notation: L 0..1, C, H in degrees) to gpui
/// [`Hsla`].
pub fn oklch(l: f32, c: f32, h_deg: f32) -> Hsla {
    let [r, g, b] = oklch_to_srgb(l, c, h_deg);
    let (h, s, l) = rgb_to_hsl(r, g, b);
    hsla(h, s, l, 1.0)
}

/// A chroma-0 oklch tone. `r == g == b` exactly, so this skips the hue math
/// entirely (avoids float-noise saturation on a color that has none).
pub fn neutral(lightness: f32) -> Hsla {
    let [v, _, _] = oklch_to_srgb(lightness, 0.0, 0.0);
    hsla(0.0, 0.0, v, 1.0)
}

/// oklch -> sRGB (each channel 0..1, gamut-clipped by clamping before gamma
/// encoding). Reference: Björn Ottosson's OKLab definition — the same
/// matrices CSS Color 4 specifies.
pub(crate) fn oklch_to_srgb(l: f32, c: f32, h_deg: f32) -> [f32; 3] {
    let h = h_deg.to_radians();
    let a = c * h.cos();
    let b = c * h.sin();

    // OKLab -> LMS (cube roots undone).
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);

    // LMS -> linear sRGB.
    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_93 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_4 * s3;
    let b = -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;

    [gamma_encode(r), gamma_encode(g), gamma_encode(b)]
}

fn gamma_encode(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB (0..1 components) -> HSL, all components 0..1 (gpui's [`Hsla`]
/// convention).
pub(crate) fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let delta = max - min;
    if delta < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = if l > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let h = if (max - r).abs() < f32::EPSILON {
        ((g - b) / delta).rem_euclid(6.0)
    } else if (max - g).abs() < f32::EPSILON {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    } / 6.0;
    (h, s, l)
}

/// HSL (gpui convention, all 0..1) -> sRGB components 0..1. Only the palette
/// tests exercise this direction; [`oklch`]/[`neutral`] only ever go the
/// other way via [`rgb_to_hsl`].
#[cfg(test)]
pub(crate) fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    if s <= f32::EPSILON {
        return [l, l, l];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hue = |mut t: f32| {
        t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0)]
}

/// Inverse of [`gamma_encode`]: gamma-encoded sRGB (0..1) -> linear sRGB,
/// for computing WCAG luminance in the palette's contrast tests.
#[cfg(test)]
fn gamma_decode(x: f32) -> f32 {
    if x <= 0.040_45 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG 2.1 relative luminance of an opaque color.
#[cfg(test)]
pub fn relative_luminance(color: Hsla) -> f32 {
    let [r, g, b] = hsl_to_rgb(color.h, color.s, color.l);
    0.2126 * gamma_decode(r) + 0.7152 * gamma_decode(g) + 0.0722 * gamma_decode(b)
}

/// WCAG 2.1 contrast ratio between two opaque colors (1.0 ..= 21.0). The
/// palette's contrast-floor tests are the only call site.
#[cfg(test)]
pub fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Linear sRGB -> OKLab (Björn Ottosson's forward transform — the exact
/// inverse direction of the LMS math in [`oklch_to_srgb`]). [`oklch_hue`] is
/// its only caller.
#[cfg(test)]
fn linear_srgb_to_oklab(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let l = 0.412_221_47 * r + 0.536_332_54 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_84 * g + 0.629_978_7 * b;
    let (l_, m_, s_) = (l.cbrt(), m.cbrt(), s.cbrt());
    let big_l = 0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_;
    let a = 1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_;
    let b2 = 0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_;
    (big_l, a, b2)
}

/// The OKLab hue (degrees, `0..360`) of the color actually **painted** —
/// i.e. after the sRGB gamut clamp every `Hsla` in this palette has already
/// been through, not the hue an out-of-gamut `oklch()` input was authored
/// with (those can differ substantially: a saturated oklch color that falls
/// outside sRGB gets its hue bent by the clamp, sometimes by tens of
/// degrees). `Hsla::h` (HSL hue) is *not* usable for a perceptual-separation
/// constraint — HSL hue is not perceptually uniform, which is the entire
/// reason this module authors colors in OKLCH in the first place; two
/// colors 20° apart in HSL can be far apart perceptually and vice versa.
/// [`tests::status_hues_are_separable`] is the reason this exists.
#[cfg(test)]
pub fn oklch_hue(color: Hsla) -> f32 {
    let [r, g, b] = hsl_to_rgb(color.h, color.s, color.l);
    let (r, g, b) = (gamma_decode(r), gamma_decode(g), gamma_decode(b));
    let (_, a, b_lab) = linear_srgb_to_oklab(r, g, b);
    b_lab.atan2(a).to_degrees().rem_euclid(360.0)
}

/// Composite `fg` (which may be translucent) over an opaque `bg`, returning
/// the opaque result — the color the eye actually receives.
/// [`tests::selection_keeps_text_readable_on_every_surface`] is the call
/// site.
#[cfg(test)]
pub fn flatten(fg: Hsla, bg: Hsla) -> Hsla {
    let a = fg.a.clamp(0.0, 1.0);
    let [fr, fg_, fb] = hsl_to_rgb(fg.h, fg.s, fg.l);
    let [br, bg_, bb] = hsl_to_rgb(bg.h, bg.s, bg.l);
    let (h, s, l) = rgb_to_hsl(
        fr * a + br * (1.0 - a),
        fg_ * a + bg_ * (1.0 - a),
        fb * a + bb * (1.0 - a),
    );
    hsla(h, s, l, 1.0)
}

// ---------------------------------------------------------------------------
// Paint helpers
//
// Free functions rather than `Theme` methods so an element builder with no
// `&Theme` in scope can still paint correctly (see the module doc).
// ---------------------------------------------------------------------------

fn ink_for(appearance: Appearance, alpha: f32) -> Hsla {
    match appearance {
        Appearance::Dark => hsla(0.0, 0.0, 1.0, alpha),
        Appearance::Light => hsla(0.0, 0.0, 0.0, alpha * INK_FILL_SCALE),
    }
}

/// Translucent **hairline** ink for borders, dividers, and rings: white on
/// dark, black on light at [`INK_HAIRLINE_SCALE`] of the alpha.
pub fn hairline(alpha: f32) -> Hsla {
    hairline_for(current_appearance(), alpha)
}

fn hairline_for(appearance: Appearance, alpha: f32) -> Hsla {
    match appearance {
        Appearance::Dark => hsla(0.0, 0.0, 1.0, alpha),
        Appearance::Light => hsla(0.0, 0.0, 0.0, (alpha * INK_HAIRLINE_SCALE).min(0.5)),
    }
}

/// Modal backdrop at `alpha_dark` (quoted in dark-mode terms). Black in both
/// appearances — a scrim darkens what's behind it, and a "light scrim" of
/// white would wash the modal out. Light scales to roughly half: a
/// dark-mode-weight scrim on a bright field reads as a blackout.
pub fn scrim(alpha_dark: f32) -> Hsla {
    match current_appearance() {
        Appearance::Dark => hsla(0.0, 0.0, 0.0, alpha_dark),
        Appearance::Light => hsla(0.0, 0.0, 0.0, 0.30 * (alpha_dark / SCRIM_ALPHA_DARK)),
    }
}

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

/// The app theme. Two concrete instances — [`Theme::dark`] and
/// [`Theme::light`] — installed as a gpui [`Global`] at boot and read with
/// [`Theme::of`].
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Which appearance these tokens were built for.
    pub appearance: Appearance,

    // ---- surfaces ----
    /// Main content plane — the worktree list lives here. Dark: the deepest
    /// plane. Light: pure white.
    pub bg: Hsla,
    /// Chrome: sidebar, titlebar, footer. Dark: one step *up* from `bg`.
    /// Light: one step *down* (grey) — chrome recedes in both appearances.
    pub surface: Hsla,
    /// Opaque pills/chips proud of the panel.
    pub surface_raised: Hsla,
    /// Hover for an opaque plate — *brighten* it (dark) or *darken* it
    /// (light); never swap it for a translucent wash.
    pub surface_raised_hover: Hsla,
    /// Recessed wells: inputs, code/diff backgrounds.
    pub surface_inset: Hsla,
    /// Modal dialog surface.
    pub surface_dialog: Hsla,
    /// Popover / menu / palette — the highest plane.
    pub surface_overlay: Hsla,
    /// Hover wash for interactive rows/buttons.
    pub element_hover: Hsla,
    /// Selected/active wash — the only thing that marks a selected row in
    /// `ui::row`. Alpha is 0.16 here vs. `element_hover`'s 0.06, so
    /// selection stays legible next to hover, unfocused, and with the
    /// pointer elsewhere, without reaching for a tinted fill.
    pub element_active: Hsla,
    /// Hairline border.
    pub border: Hsla,
    /// Stronger border for focused/raised edges.
    pub border_strong: Hsla,

    // ---- text ----
    /// Primary text.
    pub text: Hsla,
    /// Secondary labels, timestamps.
    pub text_muted: Hsla,
    /// Placeholders, disabled copy.
    pub text_faint: Hsla,
    /// Shortcut hints — the quietest tier.
    pub text_ghost: Hsla,

    // ---- accent: identity and focus only, never structural ----
    /// wtm's brand orange.
    pub accent: Hsla,
    /// Stronger accent for fills that carry [`Self::on_accent`] text.
    pub accent_strong: Hsla,
    /// Label/icon color on top of [`Self::accent_strong`].
    pub on_accent: Hsla,

    // ---- selection & caret: also identity/focus only, never structural ----
    /// Text-selection highlight band: a translucent `accent`-family wash.
    /// Pre-redesign, `text_input.rs` had no dedicated token and painted a
    /// bare `theme.accent.alpha(0.25)` at the call site (see that module's
    /// doc) — this is the same idea promoted to a real field, tuned per
    /// appearance so `Self::text` painted on top of it stays readable (see
    /// [`tests::selection_keeps_text_readable_on_every_surface`]) rather
    /// than picked once and reused blind in both appearances.
    pub selection: Hsla,
    /// The blinking text-input caret. Opaque `accent` — the same value
    /// `text_input.rs` already painted before this field existed; a caret
    /// is a 2px hairline, not a fill large enough to need its own tuning.
    pub caret: Hsla,

    // ---- status: the four facts the list is scanned for ----
    /// Dirty.
    pub warning: Hsla,
    /// Ahead / behind.
    pub info: Hsla,
    /// Gone / destructive.
    pub danger: Hsla,
    /// Destructive-action button plate; carries [`Self::on_accent`].
    pub danger_strong: Hsla,
    /// Merged / clean / OK.
    pub success: Hsla,
    /// Softer success for secondary inline copy.
    pub success_muted: Hsla,

    // ---- mono / code ----
    pub diff_add: Hsla,
    pub diff_del: Hsla,
    pub diff_add_wash: Hsla,
    pub diff_del_wash: Hsla,
    pub diff_hunk_bg: Hsla,

    // ---- fonts ----
    /// Row labels, body text, dialog titles — everything that isn't a
    /// path/SHA/branch name or diff content (see [`Self::font_mono`]).
    /// `&'static str`, not `SharedString`: `Theme` derives `Copy`, and
    /// `SharedString` only derives `Clone`, so swapping it in would silently
    /// break every call site that copies `Theme` into a closure.
    pub font_sans: &'static str,
    /// Paths, SHAs, branch names in meta position, and diff content
    /// (`ui::meta`/`ui::kbd`/`diff_view.rs`). Same `&'static str` reasoning
    /// as [`Self::font_sans`].
    pub font_mono: &'static str,

    // ---- keyboard focus ----
    /// Whether `ui.rs`'s interactive components register themselves as Tab
    /// stops; `app::WtmApp::render` forces this `false` on a copy of `Theme`
    /// for the background chrome while an overlay is open, since gpui's
    /// `tab_group()` scopes tab *order* to the dialog but doesn't stop
    /// `Window::focus_next`/`focus_prev` from walking into the shell behind it.
    pub tab_stops: bool,
}

impl Theme {
    /// The palette in effect, falling back to dark before [`init`] has run.
    pub fn of(cx: &App) -> Self {
        if cx.has_global::<ActiveTheme>() {
            cx.global::<ActiveTheme>().0
        } else {
            Self::dark()
        }
    }

    /// Build the dark theme.
    pub fn dark() -> Self {
        let appearance = Appearance::Dark;
        let bg = neutral(0.145);
        let surface = neutral(0.185);
        let text_muted = neutral(0.708);
        // `text_faint`/`text_ghost` are tuned up slightly from the SPEC's
        // suggested 0.556/0.470 — see `Theme::dark`'s doc note below.
        let text_faint = neutral(0.575);
        let text_ghost = neutral(0.505);
        // Amber-400 (`oklch(0.828, 0.189, 84.429)`) paints only ~29° from
        // `accent`'s painted hue (52.0°) in OKLab space, which reads as "two
        // oranges" on a dense list row. Retuned to yellow-400
        // (`oklch(0.852, 0.199, 91.936)`), ~36.9° from accent. See
        // `tests::status_hues_are_separable`, which measures this in OKLab
        // hue (via `oklch_hue`) rather than HSL hue — HSL hue is not
        // perceptually uniform and an earlier version of this test used it
        // by mistake.
        let warning = oklch(0.852, 0.199, 91.936);
        let danger = oklch(0.704, 0.191, 22.216);
        let success = oklch(0.765, 0.177, 163.223);
        let accent = oklch(0.750, 0.150, 52.0);

        Self {
            appearance,
            bg,
            surface,
            surface_raised: neutral(0.245),
            surface_raised_hover: neutral(0.295),
            surface_inset: neutral(0.120),
            surface_dialog: neutral(0.205),
            surface_overlay: neutral(0.235),
            element_hover: hsla(0.0, 0.0, 0.92, 0.06),
            element_active: hsla(0.0, 0.0, 0.92, 0.16),
            border: hsla(0.0, 0.0, 1.0, 0.08),
            border_strong: hsla(0.0, 0.0, 1.0, 0.14),

            text: neutral(0.922),
            text_muted,
            text_faint,
            text_ghost,

            accent,
            accent_strong: oklch(0.700, 0.180, 48.0),
            on_accent: neutral(0.145),

            // 0.28 keeps the band clearly visible against `surface_inset`
            // (a text field's usual backing) without dropping `text`'s
            // contrast on top of it below the AA floor — see
            // `tests::selection_keeps_text_readable_on_every_surface`.
            selection: accent.opacity(0.28),
            caret: accent,

            warning,
            info: oklch(0.707, 0.165, 254.624),
            danger,
            danger_strong: oklch(0.580, 0.160, 22.216),
            success,
            success_muted: oklch(0.845, 0.143, 163.223),

            diff_add: success,
            diff_del: danger,
            diff_add_wash: success.opacity(0.10),
            diff_del_wash: danger.opacity(0.10),
            diff_hunk_bg: ink_for(appearance, 0.04),

            font_sans: FONT_SANS_DEFAULT,
            font_mono: FONT_MONO_DEFAULT,

            tab_stops: true,
        }
    }

    /// Build the light theme.
    ///
    /// Neutrals are the same oklch scale read from the other end, but roles
    /// are reassigned rather than mirrored — see the module doc.
    pub fn light() -> Self {
        let appearance = Appearance::Light;
        let bg = neutral(1.0);
        let surface = neutral(0.968);
        let text_muted = neutral(0.439);
        let text_faint = neutral(0.535);
        let text_ghost = neutral(0.620);
        // The real Tailwind amber-700/red-600 values paint too close to
        // `accent`'s painted OKLab hue (~37.0°, after the sRGB gamut clamp
        // bends its authored 45.0° down — see `oklch_hue`'s doc) to read as
        // distinct from it. Both hues below are rotated further away and
        // re-solved for L/C so each still clears its contrast floor; see
        // `tests::status_hues_are_separable`, which measures the gap in
        // OKLab hue via `oklch_hue`, not `Hsla::h` (HSL hue is not
        // perceptually uniform).
        let warning = oklch(0.55, 0.13, 90.0);
        let danger = oklch(0.59, 0.24, 14.0);
        let success = oklch(0.596, 0.145, 163.225);
        let accent = oklch(0.553, 0.195, 45.0);

        Self {
            appearance,
            bg,
            surface,
            // A real grey, not white: this plate sits directly on the white
            // content plane with no border to save it.
            surface_raised: neutral(0.940),
            surface_raised_hover: neutral(0.900),
            surface_inset: neutral(0.955),
            surface_dialog: bg,
            surface_overlay: bg,
            element_hover: hsla(0.0, 0.0, 0.10, 0.06),
            element_active: hsla(0.0, 0.0, 0.10, 0.16),
            border: hsla(0.0, 0.0, 0.0, 0.10),
            border_strong: hsla(0.0, 0.0, 0.0, 0.17),

            // Not pure neutral-900 (17.9:1 on white — more contrast than
            // dark mode's, which reads harsh). 0.25 lands at ~16:1, the same
            // perceived weight as dark mode's `text`.
            text: neutral(0.25),
            text_muted,
            text_faint,
            text_ghost,

            accent,
            accent_strong: oklch(0.553, 0.195, 45.0),
            on_accent: neutral(0.985),

            // Lower than dark's 0.28: light's `accent` is darker/more
            // saturated (`oklch(0.553, ..)` vs dark's `oklch(0.750, ..)`),
            // so the same alpha would shift a white/near-white field's
            // lightness down harder — see
            // `tests::selection_keeps_text_readable_on_every_surface`.
            selection: accent.opacity(0.20),
            caret: accent,

            warning,
            info: oklch(0.488, 0.243, 264.376),
            danger,
            danger_strong: oklch(0.510, 0.200, 14.0),
            success,
            success_muted: oklch(0.510, 0.118, 163.225),

            diff_add: success,
            diff_del: danger,
            diff_add_wash: success.opacity(0.09),
            diff_del_wash: danger.opacity(0.09),
            diff_hunk_bg: ink_for(appearance, 0.04),

            font_sans: FONT_SANS_DEFAULT,
            font_mono: FONT_MONO_DEFAULT,

            tab_stops: true,
        }
    }

    /// Build the theme for an appearance.
    fn for_appearance(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Dark => Self::dark(),
            Appearance::Light => Self::light(),
        }
    }

    /// Install the theme for `appearance` as the gpui global and point the
    /// context-free paint helpers at it — the only place [`ActiveTheme`] is
    /// written outside this module.
    fn install(appearance: Appearance, cx: &mut App) {
        set_current_appearance(appearance);
        cx.set_global(ActiveTheme(Self::for_appearance(appearance)));
    }

    /// The translucent tint the sidebar/chrome paints over the blurred
    /// window backing (macOS vibrancy). Opaque (`a == 1.0`) on platforms
    /// with no compositor-blur guarantee — see [`GLASS_ALPHA`].
    pub fn glass(&self) -> Hsla {
        let alpha = match self.appearance {
            Appearance::Dark => GLASS_ALPHA,
            Appearance::Light => GLASS_ALPHA_LIGHT,
        };
        self.surface.opacity(alpha)
    }

    /// Whether this appearance paints translucent chrome over a blurred
    /// desktop. Glass-only recipes must gate on this, not on [`GLASS_ALPHA`]
    /// directly: that constant is platform-wide, this is per-appearance.
    pub fn is_glass(&self) -> bool {
        self.glass().a < 1.0
    }

    /// How the platform should composite the window behind our paint.
    /// **Must be re-applied after every theme swap** — see
    /// [`reapply_window_background`]'s doc for why.
    pub fn window_background_appearance(&self) -> gpui::WindowBackgroundAppearance {
        if self.is_glass() {
            gpui::WindowBackgroundAppearance::Blurred
        } else if cfg!(target_os = "macos") {
            gpui::WindowBackgroundAppearance::Opaque
        } else {
            // Linux keeps Transparent even when not glass — window_frame's
            // rounded client-side-decoration corners need real compositing
            // in the margin outside them (see main.rs's `WINDOW_BACKGROUND`
            // doc, whose reasoning still holds).
            gpui::WindowBackgroundAppearance::Transparent
        }
    }

    /// Menu / palette / context menu: a tight contact shadow plus a soft
    /// ambient one.
    pub fn shadow_popover(&self) -> Vec<BoxShadow> {
        let (a1, a2) = match self.appearance {
            Appearance::Light => (0.06, 0.12),
            Appearance::Dark => (0.12, 0.24),
        };
        vec![
            BoxShadow {
                color: hsla(0.0, 0.0, 0.0, a1),
                offset: point(px(0.0), px(2.0)),
                blur_radius: px(4.0),
                spread_radius: px(0.0),
            },
            BoxShadow {
                color: hsla(0.0, 0.0, 0.0, a2),
                offset: point(px(0.0), px(8.0)),
                blur_radius: px(24.0),
                spread_radius: px(0.0),
            },
        ]
    }

    /// Modal dialog: the heaviest ladder, floating over [`scrim`].
    pub fn shadow_dialog(&self) -> Vec<BoxShadow> {
        let (a1, a2) = match self.appearance {
            Appearance::Light => (0.08, 0.20),
            Appearance::Dark => (0.16, 0.40),
        };
        vec![
            BoxShadow {
                color: hsla(0.0, 0.0, 0.0, a1),
                offset: point(px(0.0), px(4.0)),
                blur_radius: px(8.0),
                spread_radius: px(0.0),
            },
            BoxShadow {
                color: hsla(0.0, 0.0, 0.0, a2),
                offset: point(px(0.0), px(16.0)),
                blur_radius: px(48.0),
                spread_radius: px(0.0),
            },
        ]
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

#[derive(Clone, Copy)]
struct ActiveTheme(Theme);

impl Global for ActiveTheme {}

/// Resolve the palette from the system appearance and publish it. Called
/// before any window exists.
pub fn init(cx: &mut App) {
    let appearance = Appearance::from_window(cx.window_appearance());
    Theme::install(appearance, cx);
}

/// Re-resolve after the system (or a forced preference) switches between
/// light and dark, and repaint every window.
///
/// Always calls [`reapply_window_background`], even though nothing here
/// detects whether the resolved palette actually changed — see that
/// function's doc for why a missed re-apply is a real (if rare) bug and an
/// unconditional call is the deliberate fix.
pub fn refresh(appearance: WindowAppearance, cx: &mut App) {
    Theme::install(Appearance::from_window(appearance), cx);
    reapply_window_background(cx);
    cx.refresh_windows();
}

/// Push [`Theme::window_background_appearance`] onto every open window.
///
/// gpui's macOS backend tears the `NSVisualEffectView` out of a window's
/// layer hierarchy the moment its background appearance is set to anything
/// other than `Blurred`, and nothing puts it back on its own. `theme::init`
/// only runs once, before any window exists, so the window's *initial*
/// `WindowOptions::window_background` (set directly in `main.rs`) is the
/// only thing establishing vibrancy at first paint — this function is what
/// keeps it alive across every later appearance swap. Called unconditionally
/// from [`refresh`]; a single missed call leaves the sidebar permanently
/// opaque until the app restarts.
pub fn reapply_window_background(cx: &mut App) {
    let Some(wanted) = cx
        .try_global::<ActiveTheme>()
        .map(|t| t.0.window_background_appearance())
    else {
        return;
    };
    for window in cx.windows() {
        let _ = window.update(cx, |_, window, _| {
            window.set_background_appearance(wanted);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn srgb_u8(c: [f32; 3]) -> [u8; 3] {
        [
            (c[0] * 255.0).round() as u8,
            (c[1] * 255.0).round() as u8,
            (c[2] * 255.0).round() as u8,
        ]
    }

    /// Circular distance between two hue angles in degrees.
    fn hue_gap(a_deg: f32, b_deg: f32) -> f32 {
        let d = (a_deg - b_deg).rem_euclid(360.0);
        d.min(360.0 - d)
    }

    // 1. -------------------------------------------------------------
    #[test]
    fn neutral_950_is_0a0a0a() {
        let rgb = srgb_u8(oklch_to_srgb(0.145, 0.0, 0.0));
        assert_eq!(rgb, [10, 10, 10]);
    }

    // 2. -------------------------------------------------------------
    #[test]
    fn oklch_accents_match_reference() {
        // Independently-computed CSS Color 4 anchors (also, conveniently,
        // this palette's own dark warning/danger tones).
        assert_eq!(
            srgb_u8(oklch_to_srgb(0.673, 0.182, 276.935)),
            [124, 134, 255]
        ); // indigo-400
        assert_eq!(
            srgb_u8(oklch_to_srgb(0.704, 0.191, 22.216)),
            [255, 100, 103]
        ); // red-400 == our dark `danger`
        assert_eq!(srgb_u8(oklch_to_srgb(0.828, 0.189, 84.429)), [255, 185, 0]);
        // amber-400 == our dark `warning`
    }

    // 3. -------------------------------------------------------------
    #[test]
    fn hsl_roundtrips_through_rgb() {
        for c in [
            Theme::dark().accent,
            Theme::dark().warning,
            Theme::light().accent,
            Theme::light().danger,
            neutral(0.556),
        ] {
            let [r, g, b] = hsl_to_rgb(c.h, c.s, c.l);
            let (h, s, l) = rgb_to_hsl(r, g, b);
            assert!((l - c.l).abs() < 1e-3, "lightness drift for {c:?}");
            assert!((s - c.s).abs() < 1e-3, "saturation drift for {c:?}");
            if c.s > 1e-3 {
                assert!((h - c.h).abs() < 1e-3, "hue drift for {c:?}");
            }
        }
    }

    // 4. -------------------------------------------------------------
    #[test]
    fn contrast_ratio_hits_known_anchors() {
        let white = hsla(0.0, 0.0, 1.0, 1.0);
        let black = hsla(0.0, 0.0, 0.0, 1.0);
        assert!((contrast_ratio(white, black) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(white, white) - 1.0).abs() < 0.01);
        assert!((contrast_ratio(black, white) - contrast_ratio(white, black)).abs() < 1e-4);
    }

    // 5. -------------------------------------------------------------
    /// Each light text token lands within 1.0 of its dark counterpart's
    /// contrast ratio against its own `bg` — a matched pair, not a mirror.
    #[test]
    fn text_contrast_is_paired_across_appearances() {
        let (d, l) = (Theme::dark(), Theme::light());
        for (name, dark_fg, light_fg) in [
            ("text", d.text, l.text),
            ("text_muted", d.text_muted, l.text_muted),
            ("text_faint", d.text_faint, l.text_faint),
        ] {
            let dr = contrast_ratio(dark_fg, d.bg);
            let lr = contrast_ratio(light_fg, l.bg);
            assert!(
                (dr - lr).abs() < 1.0,
                "{name}: dark {dr:.2}:1 vs light {lr:.2}:1 — not a matched pair"
            );
        }
    }

    // 6. -------------------------------------------------------------
    #[test]
    fn text_tones_clear_wcag_aa() {
        for t in [Theme::dark(), Theme::light()] {
            for (name, fg, floor) in [
                ("text", t.text, 4.5),
                ("text_muted", t.text_muted, 4.5),
                ("text_faint", t.text_faint, 4.1),
                ("text_ghost", t.text_ghost, 3.0),
            ] {
                let on_bg = contrast_ratio(fg, t.bg);
                let on_surface = contrast_ratio(fg, t.surface);
                assert!(
                    on_bg >= floor,
                    "{:?} {name} on bg is {on_bg:.2}:1, below {floor}",
                    t.appearance
                );
                assert!(
                    on_surface >= floor,
                    "{:?} {name} on surface is {on_surface:.2}:1, below {floor}",
                    t.appearance
                );
            }
        }
    }

    // 7. -------------------------------------------------------------
    #[test]
    fn accents_clear_contrast_on_their_background() {
        let l = Theme::light();
        assert!(
            contrast_ratio(l.accent, l.bg) >= 4.5,
            "light accent {:.2}:1",
            contrast_ratio(l.accent, l.bg)
        );
        for t in [Theme::dark(), Theme::light()] {
            for (name, c) in [
                ("warning", t.warning),
                ("info", t.info),
                ("danger", t.danger),
                ("success", t.success),
            ] {
                let on_bg = contrast_ratio(c, t.bg);
                let on_surface = contrast_ratio(c, t.surface);
                assert!(
                    on_bg >= 3.0,
                    "{:?} {name} on bg is {on_bg:.2}:1, below the 3:1 non-text floor",
                    t.appearance
                );
                assert!(
                    on_surface >= 3.0,
                    "{:?} {name} on surface is {on_surface:.2}:1, below the 3:1 non-text floor",
                    t.appearance
                );
            }
        }
    }

    // 8. -------------------------------------------------------------
    /// Measured in **OKLab hue** (`oklch_hue`, the color actually painted
    /// after the sRGB gamut clamp), not `Hsla::h` (HSL hue). HSL hue is not
    /// perceptually uniform — the entire reason this module authors in
    /// OKLCH — so a separation constraint measured in it would be
    /// meaningless: two colors 20° apart in HSL can be far apart
    /// perceptually and vice versa. An earlier version of this test used
    /// HSL hue by mistake and passed on values that read as "two oranges"
    /// next to each other in the actual rendered list.
    #[test]
    fn status_hues_are_separable() {
        for t in [Theme::dark(), Theme::light()] {
            let accent_h = oklch_hue(t.accent);
            let warning_gap = hue_gap(accent_h, oklch_hue(t.warning));
            let danger_gap = hue_gap(accent_h, oklch_hue(t.danger));
            assert!(
                warning_gap >= 30.0,
                "{:?}: accent/warning OKLab hue gap {warning_gap:.1}°, below 30°",
                t.appearance
            );
            assert!(
                danger_gap >= 20.0,
                "{:?}: accent/danger OKLab hue gap {danger_gap:.1}°, below 20°",
                t.appearance
            );
        }
    }

    // 9. -------------------------------------------------------------
    #[test]
    fn elevation_ladder_is_ordered() {
        let d = Theme::dark();
        assert!(d.bg.l < d.surface.l, "dark: bg should sit under surface");
        assert!(
            d.surface.l < d.surface_dialog.l,
            "dark: surface should sit under surface_dialog"
        );
        assert!(
            d.surface_dialog.l < d.surface_overlay.l,
            "dark: surface_dialog should sit under surface_overlay"
        );

        let l = Theme::light();
        assert!(
            l.surface.l < l.bg.l,
            "light: surface should be darker than bg"
        );
        for (name, c) in [
            ("surface_dialog", l.surface_dialog),
            ("surface_overlay", l.surface_overlay),
        ] {
            assert!(
                (c.l - l.bg.l).abs() < 1e-6,
                "light: {name} should be white, same as bg"
            );
        }
    }

    // 11. ------------------------------------------------------------
    /// `selection` sits *behind* `text` — a `TextInput` paints the band
    /// first, then shapes the line on top of it (see `text_input.rs`'s
    /// `paint`) — on whichever surface the field itself rests on: the
    /// sidebar/dialog's inset well (`surface_inset`), the command palette's
    /// overlay (`surface_overlay`), or a dialog field (`surface_dialog`).
    /// Rather than pick one surface or hand-pick a contrast number, this
    /// composites `selection` over every surface in the ladder with
    /// [`flatten`] (the color the eye actually receives) and holds every
    /// one of them to the same 4.5:1 AA body-text floor
    /// [`tests::text_tones_clear_wcag_aa`] uses for `text`/`text_muted`.
    #[test]
    fn selection_keeps_text_readable_on_every_surface() {
        for t in [Theme::dark(), Theme::light()] {
            for (name, surface) in [
                ("bg", t.bg),
                ("surface", t.surface),
                ("surface_inset", t.surface_inset),
                ("surface_dialog", t.surface_dialog),
                ("surface_overlay", t.surface_overlay),
            ] {
                let composited = flatten(t.selection, surface);
                let ratio = contrast_ratio(t.text, composited);
                assert!(
                    ratio >= 4.5,
                    "{:?} text-on-selection over {name} is {ratio:.2}:1, below the 4.5:1 AA floor",
                    t.appearance
                );
            }
        }
    }

    // 12. ------------------------------------------------------------
    /// `caret` is a 2px opaque fill, not text underneath it — the 3:1
    /// non-text-UI floor [`tests::accents_clear_contrast_on_their_background`]
    /// already applies to the status colors, checked against every surface
    /// a text field can rest on (same ladder as the selection test above).
    #[test]
    fn caret_clears_non_text_contrast_on_every_surface() {
        for t in [Theme::dark(), Theme::light()] {
            for (name, surface) in [
                ("bg", t.bg),
                ("surface", t.surface),
                ("surface_inset", t.surface_inset),
                ("surface_dialog", t.surface_dialog),
                ("surface_overlay", t.surface_overlay),
            ] {
                let ratio = contrast_ratio(t.caret, surface);
                assert!(
                    ratio >= 3.0,
                    "{:?} caret on {name} is {ratio:.2}:1, below the 3:1 non-text floor",
                    t.appearance
                );
            }
        }
    }

    #[test]
    fn flatten_composites_translucent_over_opaque() {
        let bg = hsla(0.0, 0.0, 0.0, 1.0);
        let fg = hsla(0.0, 0.0, 1.0, 0.5);
        let flattened = flatten(fg, bg);
        assert!((flattened.a - 1.0).abs() < 1e-6, "result is opaque");
        // Half-alpha white over black should land near 50% grey.
        assert!(
            (flattened.l - 0.5).abs() < 0.05,
            "flattened lightness {}",
            flattened.l
        );
    }

    #[test]
    fn shadow_ladders_grow_with_elevation() {
        let t = Theme::dark();
        // The dialog's ambient layer should be at least as dark as the
        // popover's — the heavier ladder never gets a LIGHTER shadow.
        assert!(t.shadow_dialog()[1].color.a >= t.shadow_popover()[1].color.a);
    }

    #[test]
    fn appearance_mirror_round_trips() {
        let _guard = lock_appearance();
        set_current_appearance(Appearance::Light);
        assert_eq!(current_appearance(), Appearance::Light);
        assert_eq!(
            ink_for(current_appearance(), 1.0),
            hsla(0.0, 0.0, 0.0, 1.0 * INK_FILL_SCALE)
        );
        set_current_appearance(Appearance::Dark);
        assert_eq!(current_appearance(), Appearance::Dark);
        assert_eq!(ink_for(current_appearance(), 1.0), hsla(0.0, 0.0, 1.0, 1.0));
    }
}

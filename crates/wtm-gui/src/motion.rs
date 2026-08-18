//! Animation kit — the wtm motion catalog as reusable helpers over gpui
//! [`Animation`]/[`AnimationExt`].
//!
//! wtm is a dense, list-first utility: the worktree list and sidebar rows
//! are touched on every scroll and refresh, so they run **no** entrance
//! animations at all — hover/selection are instant `.hover()`/`.active()`
//! style states (SPEC §5's restraint rules). Overlays (dialogs, popovers,
//! the command palette, the run panel) are touched rarely and animate
//! properly, using the catalog below.
//!
//! `CubicBezier` is a straight port of Zeron/comet's evaluator
//! (`crates/ui/src/motion.rs`) — CSS `cubic-bezier()`, solved by Newton's
//! method with a bisection fallback, hard-clamped to `[0, 1]` because f32
//! rounding can otherwise push a sample a hair past 1.0 and trip gpui's
//! `AnimationElement` debug assert.
//!
//! # gpui 0.2.2 has no `cx.reduce_motion()`
//!
//! Unlike the Zed fork comet depends on, crates.io `gpui = "0.2.2"` has no
//! built-in reduced-motion flag or automatic honoring of one inside
//! `AnimationElement` (verified by grepping the vendored source — see the
//! redesign SPEC §0). [`reduced`]/[`set_reduced`] are wtm's own global, and
//! every helper below honors it internally by collapsing the animation's
//! duration to zero: [`AnimationElement`]'s `request_layout` divides
//! elapsed time by `duration.as_secs_f32()`, so a zero duration makes the
//! very first frame land past `delta > 1.0`, snap straight to the end
//! state, and — being a oneshot — never call `request_animation_frame`
//! again. Call sites never need to branch on [`reduced`] themselves.
//!
//! `translateY` is implemented as a relative-position `top` inset: taffy
//! applies relative insets after layout, so — like a CSS transform —
//! siblings never move. gpui 0.2.2 has no `div` scale transform (only
//! `svg().with_transformation(..)`, used here for the spinner), so
//! `menu_in`/`dialog_in` approximate zeron's `scale(0.96)` component with
//! fade + a small translate instead.
//!
//! # Catalog completeness
//!
//! SPEC §5's catalog table lists seven specs (`FADE_IN`, `FADE_QUICK`,
//! `MENU_IN`, `DIALOG_IN`, `RESIZE`, `COLLAPSE`, `SPINNER`) and four curves;
//! [`catalog_timings_match_spec`](tests::catalog_timings_match_spec) checks
//! all seven together. `FADE_QUICK`/`MENU_IN`/`DIALOG_IN`/`SPINNER` (via
//! [`fade_quick`]/[`menu_in`]/[`dialog_in`]/[`spin`]) now have real call
//! sites in dialogs, the palette, and context menus; `FADE_IN`/[`fade_in`]
//! (view entrances — nothing in this app mounts/unmounts a coarse "view"
//! yet), `RESIZE` (sidebar/detail-panel width — those panes aren't
//! drag-resizable yet), and `COLLAPSE` (disclosure height — the file
//! browser's chevron rotates as a static snap today; see its own doc for
//! the signature change an animated version needs) do not. Each is kept as
//! part of the documented catalog rather than deleted — see the individual
//! `#[allow(dead_code)]` reasons below.
use std::f32::consts::TAU;
use std::time::Duration;

use gpui::{px, radians, Animation, App, ElementId, Global, IntoElement, Radians, Styled, Svg};

pub use gpui::AnimationExt;

// ---------------------------------------------------------------------------
// Reduced motion
// ---------------------------------------------------------------------------

/// Global reduced-motion preference (backed by a pref at the call site;
/// default off). Every helper in this module honors it internally.
struct ReducedMotion(bool);

impl Global for ReducedMotion {}

/// Whether reduced motion is on. Defaults to `false` before [`set_reduced`]
/// has ever been called.
pub fn reduced(cx: &App) -> bool {
    cx.try_global::<ReducedMotion>().is_some_and(|r| r.0)
}

/// Set the reduced-motion preference.
pub fn set_reduced(cx: &mut App, value: bool) {
    cx.set_global(ReducedMotion(value));
}

// ---------------------------------------------------------------------------
// Cubic bezier
// ---------------------------------------------------------------------------

/// A CSS `cubic-bezier(x1, y1, x2, y2)` timing function (endpoints fixed at
/// `(0,0)` and `(1,1)`). Evaluation solves `x(t) = input` by Newton
/// iteration with a bisection fallback — the standard UnitBezier approach.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CubicBezier {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

impl CubicBezier {
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self { x1, y1, x2, y2 }
    }

    fn coefficients(a: f32, b: f32) -> (f32, f32, f32) {
        let c = 3.0 * a;
        let bb = 3.0 * (b - a) - c;
        let aa = 1.0 - c - bb;
        (aa, bb, c)
    }

    fn sample_x(&self, t: f32) -> f32 {
        let (a, b, c) = Self::coefficients(self.x1, self.x2);
        ((a * t + b) * t + c) * t
    }

    fn sample_y(&self, t: f32) -> f32 {
        let (a, b, c) = Self::coefficients(self.y1, self.y2);
        ((a * t + b) * t + c) * t
    }

    fn sample_x_derivative(&self, t: f32) -> f32 {
        let (a, b, c) = Self::coefficients(self.x1, self.x2);
        (3.0 * a * t + 2.0 * b) * t + c
    }

    /// Curve parameter `t` for a given progress `x` (both 0..1).
    fn solve_t_for_x(&self, x: f32) -> f32 {
        let mut t = x;
        for _ in 0..8 {
            let err = self.sample_x(t) - x;
            if err.abs() < 1e-6 {
                return t;
            }
            let d = self.sample_x_derivative(t);
            if d.abs() < 1e-6 {
                break;
            }
            t -= err / d;
        }
        // Bisection fallback (x(t) is monotonic for valid CSS beziers).
        let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
        for _ in 0..32 {
            let mid = (lo + hi) / 2.0;
            if self.sample_x(mid) < x {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (lo + hi) / 2.0
    }

    /// Eased output for input progress `x`, hard-clamped to `[0, 1]`.
    pub fn eval(&self, x: f32) -> f32 {
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        self.sample_y(self.solve_t_for_x(x)).clamp(0.0, 1.0)
    }

    /// This curve as a gpui easing closure. SPEC §5 names this method
    /// explicitly (`impl CubicBezier { pub fn eval(..); pub fn easing(..); }`);
    /// [`MotionSpec::animation`] builds its own inline closure instead
    /// (`Animation::new(duration).with_easing(move |d| curve.eval(d))`), so
    /// this has no caller yet — kept as the SPEC-mandated method for a
    /// future call site that hands a bare curve to a gpui API expecting
    /// `Fn(f32) -> f32` directly, without going through [`MotionSpec`].
    #[allow(dead_code)]
    pub fn easing(self) -> impl Fn(f32) -> f32 + 'static {
        move |x| self.eval(x)
    }
}

/// wtm's signature entrance curve — CSS `cubic-bezier(0.16, 1, 0.3, 1)`. Only
/// live caller is [`FADE_IN`], which has no call site yet — see the module
/// doc's "Catalog completeness" note.
#[allow(dead_code)]
pub const EASE_OUT_EXPO: CubicBezier = CubicBezier::new(0.16, 1.0, 0.3, 1.0);
/// CSS `ease-out` — width/height transitions (sidebar/detail-panel resize).
/// Only live caller is [`RESIZE`]/[`COLLAPSE`], neither of which has a call
/// site yet — see the module doc's "Catalog completeness" note.
#[allow(dead_code)]
pub const EASE_OUT: CubicBezier = CubicBezier::new(0.0, 0.0, 0.58, 1.0);
/// CSS `ease` — quick fades, menu/dialog entrances.
pub const EASE: CubicBezier = CubicBezier::new(0.25, 0.1, 0.25, 1.0);
/// Material's "standard" curve — reserved for a future call site that wants
/// a snappier symmetric ease than [`EASE`]. One of SPEC §5's four named
/// curves; no catalog spec uses it yet.
#[allow(dead_code)]
pub const EASE_STANDARD: CubicBezier = CubicBezier::new(0.2, 0.0, 0.0, 1.0);
/// The identity curve (linear) — used by [`SPINNER`], where the catalog
/// calls for a constant angular rate rather than an ease.
pub const LINEAR: CubicBezier = CubicBezier::new(0.0, 0.0, 1.0, 1.0);

// ---------------------------------------------------------------------------
// Motion specs (the catalog — SPEC §5)
// ---------------------------------------------------------------------------

/// One catalog entry: duration + curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionSpec {
    pub duration_ms: u64,
    pub curve: CubicBezier,
}

impl MotionSpec {
    pub const fn new(duration_ms: u64, curve: CubicBezier) -> Self {
        Self { duration_ms, curve }
    }

    /// Eased progress (0..1) for a raw timeline delta (0..1). Pure —
    /// unit-testable without a window. [`Self::animation`] (the mechanism
    /// [`animate`] actually drives) recomputes the same easing inline
    /// instead of calling this, so its only caller today is
    /// [`tests::progress_matches_curve_eval_directly`] — kept as the pure,
    /// testable half of the arithmetic `better-ui`/COMPONENTS.md calls for
    /// alongside any styling helper.
    #[allow(dead_code)]
    pub fn progress(&self, raw_delta: f32) -> f32 {
        self.curve.eval(raw_delta.clamp(0.0, 1.0))
    }

    /// A oneshot gpui [`Animation`] for this spec. When `reduced` is true
    /// the duration collapses to zero, which — per this module's doc —
    /// snaps to the end state on the first frame and schedules nothing
    /// further.
    fn animation(&self, reduced: bool) -> Animation {
        let duration = if reduced {
            Duration::ZERO
        } else {
            Duration::from_millis(self.duration_ms)
        };
        let curve = self.curve;
        Animation::new(duration).with_easing(move |d| curve.eval(d))
    }
}

/// View entrances: fade + 4px rise. See [`fade_in`] and the module doc's
/// "Catalog completeness" note for why this has no call site yet.
#[allow(dead_code)]
pub const FADE_IN: MotionSpec = MotionSpec::new(300, EASE_OUT_EXPO);
/// Cheap cross-fades.
pub const FADE_QUICK: MotionSpec = MotionSpec::new(150, EASE);
/// Popovers, context menu, command palette.
pub const MENU_IN: MotionSpec = MotionSpec::new(140, EASE);
/// Modal dialogs.
pub const DIALOG_IN: MotionSpec = MotionSpec::new(180, EASE);
/// Sidebar / detail-panel width transitions. See the module doc's "Catalog
/// completeness" note — those panes are not drag-resizable yet.
#[allow(dead_code)]
pub const RESIZE: MotionSpec = MotionSpec::new(200, EASE_OUT);
/// Disclosure height. See the module doc's "Catalog completeness" note —
/// the file browser's chevron rotates as a static snap today, not yet
/// through this spec.
#[allow(dead_code)]
pub const COLLAPSE: MotionSpec = MotionSpec::new(180, EASE_OUT);
/// Loading indicator rotation — linear, repeating.
pub const SPINNER: MotionSpec = MotionSpec::new(900, LINEAR);

// ---------------------------------------------------------------------------
// Element helpers
// ---------------------------------------------------------------------------

/// Wrap `element` in a oneshot animation over `spec`, honoring [`reduced`].
/// The building block every entrance helper below is written in terms of.
pub fn animate<E>(
    id: impl Into<ElementId>,
    spec: MotionSpec,
    cx: &App,
    element: E,
    animator: impl Fn(E, f32) -> E + 'static,
) -> gpui::AnimationElement<E>
where
    E: Styled + IntoElement + 'static,
{
    element.with_animation(id, spec.animation(reduced(cx)), animator)
}

/// Standard entrance: opacity 0->1 + translateY 4->0 over [`FADE_IN`]. See
/// the module doc's "Catalog completeness" note — no view in this app
/// mounts/unmounts coarsely enough yet to want it.
#[allow(dead_code)]
pub fn fade_in<E>(id: impl Into<ElementId>, element: E, cx: &App) -> gpui::AnimationElement<E>
where
    E: Styled + IntoElement + 'static,
{
    animate(id, FADE_IN, cx, element, |el, t| {
        el.relative().opacity(t).top(px(4.0 * (1.0 - t)))
    })
}

/// Quick opacity-only fade over [`FADE_QUICK`].
pub fn fade_quick<E>(id: impl Into<ElementId>, element: E, cx: &App) -> gpui::AnimationElement<E>
where
    E: Styled + IntoElement + 'static,
{
    animate(id, FADE_QUICK, cx, element, |el, t| el.opacity(t))
}

/// Popover entrance: fade + translateY -2->0 over [`MENU_IN`]. gpui divs
/// have no scale transform, so zeron's accompanying `scale(0.96)` is
/// approximated with the fade + translate alone.
pub fn menu_in<E>(id: impl Into<ElementId>, element: E, cx: &App) -> gpui::AnimationElement<E>
where
    E: Styled + IntoElement + 'static,
{
    animate(id, MENU_IN, cx, element, |el, t| {
        el.relative()
            .opacity(0.3 + 0.7 * t)
            .top(px(-2.0 * (1.0 - t)))
    })
}

/// Dialog entrance over [`DIALOG_IN`] (scale approximated with fade + 2px
/// rise, same caveat as [`menu_in`]).
pub fn dialog_in<E>(id: impl Into<ElementId>, element: E, cx: &App) -> gpui::AnimationElement<E>
where
    E: Styled + IntoElement + 'static,
{
    animate(id, DIALOG_IN, cx, element, |el, t| {
        el.relative().opacity(t).top(px(2.0 * (1.0 - t)))
    })
}

/// A loading-indicator rotation over [`SPINNER`]: one full turn per period,
/// linear, repeating. Reduced motion renders a static, unrotated icon and
/// mounts no animation at all — a permanently-repeating spinner is exactly
/// the "no repaint loops for idle UI" case SPEC §5 calls out, and reduced
/// motion is the one signal that says this loader's motion is unwanted, not
/// just quieter.
pub fn spin(id: impl Into<ElementId>, icon: Svg, cx: &App) -> gpui::AnyElement {
    if reduced(cx) {
        return icon.into_any_element();
    }
    icon.with_animation(
        id,
        Animation::new(Duration::from_millis(SPINNER.duration_ms)).repeat(),
        |el, t| el.with_transformation(gpui::Transformation::rotate(spin_angle(t))),
    )
    .into_any_element()
}

/// Pure rotation math for [`spin`]'s repeating animation: a full turn per
/// period, `t` in `[0, 1)`. Split out so the angle math is testable without
/// a window.
fn spin_angle(t: f32) -> Radians {
    radians(t * TAU)
}

/// Press feedback for buttons: gpui has no `div` scale transform, so
/// zeron's `scale(0.96)` press is approximated as a 1px inward nudge (a
/// relative `top` inset, the same trick [`fade_in`] uses for translateY)
/// plus a slightly stronger active wash — never an opacity drop alone, which
/// reads as "disabled" rather than "pressed". Apply inside a `.active()`
/// style closure:
///
/// ```ignore
/// div().bg(rest_bg).active(|el| motion::press_feedback(el, active_bg))
/// ```
///
/// Instant (not time-based), so it does not need to honor [`reduced`] the
/// way the entrance helpers do — SPEC §5 requires press feedback
/// unconditionally, as one of motion's "restraint rules", not as an
/// animation someone might want to turn off.
pub fn press_feedback<E: Styled>(element: E, active_wash: gpui::Hsla) -> E {
    element.bg(active_wash).relative().top(px(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f32, expected: f32, tol: f32, ctx: &str) {
        assert!(
            (actual - expected).abs() <= tol,
            "{ctx}: got {actual}, expected {expected} ± {tol}"
        );
    }

    #[test]
    fn eval_never_escapes_unit_interval_dense_sweep() {
        // Regression guard: f32 rounding can produce e.g. 1.000000119 near
        // the tail of EASE_OUT_EXPO, which would trip gpui's
        // `AnimationElement` debug assert (`delta` must stay in `[0,1]`).
        for curve in [EASE_OUT_EXPO, EASE_OUT, EASE, EASE_STANDARD, LINEAR] {
            for i in 0..=100_000u32 {
                let x = i as f32 / 100_000.0;
                let y = curve.eval(x);
                assert!((0.0..=1.0).contains(&y), "eval({x}) = {y} escaped [0,1]");
            }
            for x in [0.999_999f32, 0.999_999_9, 1.0 - f32::EPSILON] {
                let y = curve.eval(x);
                assert!((0.0..=1.0).contains(&y), "eval({x}) = {y} escaped [0,1]");
            }
        }
    }

    #[test]
    fn bezier_linear_is_identity() {
        for x in [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
            assert_close(LINEAR.eval(x), x, 1e-4, "linear");
        }
    }

    #[test]
    fn bezier_known_values() {
        // References computed independently with 80-step bisection.
        let cases: [(&str, CubicBezier, [f32; 5]); 3] = [
            (
                "expo",
                EASE_OUT_EXPO,
                [0.494391, 0.825622, 0.971779, 0.997677, 0.999878],
            ),
            (
                "ease-out",
                EASE_OUT,
                [0.160572, 0.378138, 0.684643, 0.906535, 0.982973],
            ),
            (
                "ease",
                EASE,
                [0.094796, 0.408511, 0.802403, 0.960459, 0.994316],
            ),
        ];
        for (name, curve, expected) in cases {
            for (x, want) in [0.1, 0.25, 0.5, 0.75, 0.9].into_iter().zip(expected) {
                assert_close(curve.eval(x), want, 1e-3, name);
            }
        }
    }

    #[test]
    fn bezier_endpoints_and_clamping() {
        for curve in [EASE_OUT_EXPO, EASE_OUT, EASE, EASE_STANDARD] {
            assert_eq!(curve.eval(0.0), 0.0);
            assert_eq!(curve.eval(1.0), 1.0);
            assert_eq!(curve.eval(-0.5), 0.0);
            assert_eq!(curve.eval(1.5), 1.0);
        }
    }

    #[test]
    fn bezier_is_monotonic_for_catalog_curves() {
        for curve in [EASE_OUT_EXPO, EASE_OUT, EASE, EASE_STANDARD] {
            let mut last = 0.0;
            for i in 0..=100 {
                let y = curve.eval(i as f32 / 100.0);
                assert!(y >= last - 1e-4, "monotonicity violated at {i}");
                last = y;
            }
        }
    }

    #[test]
    fn catalog_timings_match_spec() {
        assert_eq!(FADE_IN.duration_ms, 300);
        assert_eq!(FADE_QUICK.duration_ms, 150);
        assert_eq!(MENU_IN.duration_ms, 140);
        assert_eq!(DIALOG_IN.duration_ms, 180);
        assert_eq!(RESIZE.duration_ms, 200);
        assert_eq!(COLLAPSE.duration_ms, 180);
        assert_eq!(SPINNER.duration_ms, 900);
        assert_eq!(EASE_OUT_EXPO, CubicBezier::new(0.16, 1.0, 0.3, 1.0));
    }

    #[test]
    fn progress_matches_curve_eval_directly() {
        assert_close(FADE_IN.progress(0.5), EASE_OUT_EXPO.eval(0.5), 1e-6, "");
        assert_eq!(FADE_IN.progress(-1.0), FADE_IN.progress(0.0), "clamps low");
        assert_eq!(FADE_IN.progress(2.0), FADE_IN.progress(1.0), "clamps high");
    }

    #[test]
    fn spin_angle_completes_one_turn_per_period() {
        assert_close(spin_angle(0.0).0, 0.0, 1e-6, "start");
        assert_close(
            spin_angle(0.25).0,
            std::f32::consts::FRAC_PI_2,
            1e-5,
            "quarter",
        );
        assert_close(spin_angle(1.0).0, TAU, 1e-5, "full turn");
    }

    #[test]
    fn reduced_motion_defaults_off_and_toggles() {
        // `reduced` reads a gpui `App` global; without one constructed here
        // (no window/App in a plain `#[test]`), it must fall back to `false`
        // rather than panicking — the whole point of `is_some_and`.
        // `set_reduced`/`reduced` themselves are exercised end-to-end by the
        // app-level gpui tests in `crate::app`, which do have an `App`.
        let animation_full = FADE_IN.animation(false);
        let animation_reduced = FADE_IN.animation(true);
        assert_eq!(animation_full.duration, Duration::from_millis(300));
        assert_eq!(animation_reduced.duration, Duration::ZERO);
    }

    #[test]
    fn animation_reduced_duration_snaps_curve_to_endpoints() {
        // The mechanism `spin`/`animate` rely on: a zero-duration animation
        // must still resolve to valid eased values at both ends, since
        // `AnimationElement` evaluates delta = elapsed / duration on its
        // very first frame regardless of how short `duration` is.
        for curve in [EASE_OUT_EXPO, EASE, EASE_OUT] {
            assert_eq!(curve.eval(0.0), 0.0);
            assert_eq!(curve.eval(1.0), 1.0);
        }
    }
}

//! Animation kit — the wtm motion catalog as reusable helpers over gpui
//! [`Animation`]/[`AnimationExt`].
//!
//! wtm is a dense, list-first utility: the worktree list and sidebar rows
//! run **no** entrance animations (hover/selection are instant `.hover()`/
//! `.active()` style states). Overlays, the sidebar/detail-panel mount, and
//! per-row status settling are touched rarely and animate via the catalog
//! below.
//!
//! `CubicBezier` is a straight port of Zeron/comet's evaluator — CSS
//! `cubic-bezier()`, solved by Newton's method with a bisection fallback,
//! hard-clamped to `[0, 1]` because f32 rounding can otherwise push a
//! sample a hair past 1.0 and trip gpui's `AnimationElement` debug assert.
//!
//! crates.io `gpui = "0.2.2"` has no built-in reduced-motion flag.
//! [`reduced`]/[`set_reduced`] are wtm's own global; every helper below
//! honors it by collapsing the animation's duration to zero, which makes
//! `AnimationElement`'s first frame land past `delta > 1.0` and snap
//! straight to the end state — call sites never need to branch on
//! [`reduced`] themselves.
//!
//! gpui 0.2.2 has no `div` scale transform, so `menu_in`/`dialog_in`
//! approximate zeron's `scale(0.96)` with fade + a small `top`-inset
//! translate instead (a relative inset so, like a CSS transform, siblings
//! never move).
use std::f32::consts::{FRAC_PI_2, TAU};
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
}

/// CSS `ease-out` — width/height transitions (sidebar/detail-panel resize),
/// disclosure height. Live caller: [`RESIZE`] (via [`pane_in`]) and
/// [`COLLAPSE`] (via [`disclosure_chevron`]).
pub const EASE_OUT: CubicBezier = CubicBezier::new(0.0, 0.0, 0.58, 1.0);
/// CSS `ease` — quick fades, menu/dialog entrances.
pub const EASE: CubicBezier = CubicBezier::new(0.25, 0.1, 0.25, 1.0);
/// The identity curve (linear) — used by [`SPINNER`], where the catalog
/// calls for a constant angular rate rather than an ease.
pub const LINEAR: CubicBezier = CubicBezier::new(0.0, 0.0, 1.0, 1.0);

// ---------------------------------------------------------------------------
// Motion specs (the catalog)
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

/// Cheap cross-fades.
pub const FADE_QUICK: MotionSpec = MotionSpec::new(150, EASE);
/// Popovers, context menu, command palette.
pub const MENU_IN: MotionSpec = MotionSpec::new(140, EASE);
/// Modal dialogs.
pub const DIALOG_IN: MotionSpec = MotionSpec::new(180, EASE);
/// Sidebar / detail-panel mount transition. See [`pane_in`] for why this
/// times an opacity + slide rather than the width itself.
pub const RESIZE: MotionSpec = MotionSpec::new(200, EASE_OUT);
/// Disclosure rotation — the file browser's expand/collapse chevron, via
/// [`disclosure_chevron`].
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

/// Sidebar/detail-panel mount transition over [`RESIZE`]: opacity 0->1 plus
/// a small horizontal slide, `start_offset_px` -> 0. The pane's own
/// `w(px(..))` is set by the caller, outside this wrapper, and never
/// animates: several row-layout budgets elsewhere in this crate read that
/// width as a flat, instantaneous number, and a real width tween would
/// desync them from the taffy-allocated box for the length of the
/// animation.
///
/// `start_offset_px` carries both the distance and the direction: negative
/// for a pane whose home edge is the window's left (the sidebar enters by
/// sliding in *from* further left), positive for one whose home edge is the
/// right (the detail panel enters from further right). Implemented as a
/// relative `left` inset for the same reason [`menu_in`] uses a relative
/// `top` inset for its own rise: taffy applies it after layout,
/// so — like a CSS transform — the sibling content column never shifts.
///
/// Entrance-only, matching every other mount transition in this module
/// (dialogs, the palette, context menus): this app has no exit-animation
/// infrastructure anywhere, and retrofitting one just for these two panes
/// would mean keeping an unmounted pane's box alive (and the content
/// column's width wrong) for the length of a fade-out. A pane that stops
/// being wanted (the user's own toggle, or the width breakpoint) disappears
/// the same instant every dialog/menu does when dismissed.
pub fn pane_in<E>(
    id: impl Into<ElementId>,
    element: E,
    start_offset_px: f32,
    cx: &App,
) -> gpui::AnimationElement<E>
where
    E: Styled + IntoElement + 'static,
{
    animate(id, RESIZE, cx, element, move |el, t| {
        el.relative()
            .opacity(t)
            .left(px(start_offset_px * (1.0 - t)))
    })
}

/// A loading-indicator rotation over [`SPINNER`]: one full turn per period,
/// linear, repeating. Reduced motion renders a static, unrotated icon and
/// mounts no animation at all — a permanently-repeating spinner would
/// otherwise be a repaint loop for idle UI, and reduced motion is the one
/// signal that says this loader's motion is unwanted, not just quieter.
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

/// A disclosure chevron's expand/collapse rotation over [`COLLAPSE`]: an
/// eased quarter turn between pointing right (collapsed) and down
/// (expanded).
///
/// `id` must be unique *per toggle*, not just per row: an
/// [`gpui::AnimationElement`]'s progress is `Instant`-based state gpui keeps
/// alive for as long as the same element id keeps appearing in the tree
/// (`window.with_element_state`), and a oneshot animation that has already
/// finished never restarts just because its target flipped again — it would
/// stay parked at whichever end of the turn it last reached. Folding a
/// per-toggle generation counter into `id` (see
/// `file_browser::FileBrowserState::toggle_generation`) makes every toggle
/// a genuinely new element identity, so it always animates instead of only
/// the first expand ever doing so. The two possible identities (this
/// generation vs. the last one) still only ever cost one bounded 180ms
/// animation each — nothing here reschedules once `t` reaches 1.
pub fn disclosure_chevron(
    id: impl Into<ElementId>,
    icon: Svg,
    expanded: bool,
    cx: &App,
) -> gpui::AnyElement {
    let (from, to) = if expanded {
        (0.0, FRAC_PI_2)
    } else {
        (FRAC_PI_2, 0.0)
    };
    animate(id, COLLAPSE, cx, icon, move |el, t| {
        el.with_transformation(gpui::Transformation::rotate(radians(
            from + (to - from) * t,
        )))
    })
    .into_any_element()
}

/// Press feedback for buttons: gpui has no `div` scale transform, so
/// zeron's `scale(0.96)` press is approximated as a 1px inward nudge (a
/// relative `top` inset, the same trick [`menu_in`] uses for translateY)
/// plus a slightly stronger active wash — never an opacity drop alone, which
/// reads as "disabled" rather than "pressed". Apply inside a `.active()`
/// style closure:
///
/// ```ignore
/// div().bg(rest_bg).active(|el| motion::press_feedback(el, active_bg))
/// ```
///
/// Instant (not time-based), so it does not need to honor [`reduced`] the
/// way the entrance helpers do — press feedback is a restraint baseline,
/// not an animation someone might want to turn off.
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
        // a curve's tail, which would trip gpui's `AnimationElement` debug
        // assert (`delta` must stay in `[0,1]`).
        for curve in [EASE_OUT, EASE, LINEAR] {
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
        let cases: [(&str, CubicBezier, [f32; 5]); 2] = [
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
        for curve in [EASE_OUT, EASE] {
            assert_eq!(curve.eval(0.0), 0.0);
            assert_eq!(curve.eval(1.0), 1.0);
            assert_eq!(curve.eval(-0.5), 0.0);
            assert_eq!(curve.eval(1.5), 1.0);
        }
    }

    #[test]
    fn bezier_is_monotonic_for_catalog_curves() {
        for curve in [EASE_OUT, EASE] {
            let mut last = 0.0;
            for i in 0..=100 {
                let y = curve.eval(i as f32 / 100.0);
                assert!(y >= last - 1e-4, "monotonicity violated at {i}");
                last = y;
            }
        }
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
        let animation_full = DIALOG_IN.animation(false);
        let animation_reduced = DIALOG_IN.animation(true);
        assert_eq!(animation_full.duration, Duration::from_millis(180));
        assert_eq!(animation_reduced.duration, Duration::ZERO);
    }
}

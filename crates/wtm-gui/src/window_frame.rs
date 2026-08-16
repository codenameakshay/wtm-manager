//! Client-side window decorations: the shadow margin, rounded corners,
//! border, and resize handles drawn around the app's own content when the
//! compositor asks the app to draw its own frame instead of a server-side
//! one (`Decorations::Client` — the common case on Wayland, and on X11
//! window managers that honor the request made in `main.rs`'s
//! `request_decorations` call). Under `Decorations::Server` — always true
//! on macOS, since gpui's mac backend never reports anything else, and true
//! on Linux whenever the compositor insists on drawing its own frame — this
//! module draws nothing and hands the content back untouched, since adding
//! a second frame on top of the compositor's own would double up the
//! chrome. That single runtime check is what keeps this module free of any
//! `#[cfg(target_os = ...)]` of its own: macOS simply never takes the
//! `Client` branch.
//!
//! The one piece of real logic here — which corners get rounded and which
//! edges get a resize handle for a given tiling state — is kept as a pure
//! function of [`gpui::Tiling`] ([`FrameLayout::for_tiling`]) specifically
//! so it can be unit-tested without a live window (see the tests below): a
//! tiled edge is pinned flush against the screen edge or a neighboring
//! tiled window, so it can neither round into a corner nor be dragged, and
//! a corner survives only when *both* edges that meet there are free.

use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, CursorStyle, Decorations, Div, IntoElement, MouseButton, Pixels,
    ResizeEdge, Stateful, Tiling, Window,
};

use crate::theme::Theme;
use crate::ui;

/// Width of the margin reserved for the drop shadow and the resize handles
/// that live in it — also what `Window::set_client_inset` is told, so the
/// compositor excludes this band from its own notion of the window's
/// content area. Matches the shadow size gpui's own `window_shadow` example
/// uses.
const INSET: Pixels = px(10.0);
/// The frame's visible 1px border, drawn just inside the shadow margin.
const BORDER: Pixels = px(1.0);

/// Which of the frame's four edges may show a resize handle, and which of
/// its four corners may be rounded, derived from the compositor's current
/// edge-tiling state. See the module doc for the reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameLayout {
    pub edge_top: bool,
    pub edge_right: bool,
    pub edge_bottom: bool,
    pub edge_left: bool,
    pub corner_top_left: bool,
    pub corner_top_right: bool,
    pub corner_bottom_left: bool,
    pub corner_bottom_right: bool,
}

impl FrameLayout {
    pub fn for_tiling(tiling: Tiling) -> Self {
        let edge_top = !tiling.top;
        let edge_right = !tiling.right;
        let edge_bottom = !tiling.bottom;
        let edge_left = !tiling.left;
        Self {
            edge_top,
            edge_right,
            edge_bottom,
            edge_left,
            // A corner is only ever free — roundable, draggable — when
            // neither of the two edges that meet there is tiled; one tiled
            // edge is enough to square it off, the same rule gpui's own
            // `window_shadow` example applies to its corner rounding.
            corner_top_left: edge_top && edge_left,
            corner_top_right: edge_top && edge_right,
            corner_bottom_left: edge_bottom && edge_left,
            corner_bottom_right: edge_bottom && edge_right,
        }
    }
}

/// Wrap `content` in the client-side decoration frame when the compositor
/// asked the app to draw its own (`Decorations::Client`): a shadow margin,
/// a rounded border on every untiled edge, and a resize handle on every
/// untiled edge and corner. Under `Decorations::Server` this returns
/// `content` unchanged — see the module doc for why that already covers
/// macOS without any `#[cfg]` here.
///
/// Also tells the compositor the shadow margin's width via
/// `set_client_inset` whenever the frame is actually drawn: it has to be
/// current before this frame paints, since a stale inset would let the
/// compositor's own hit-testing of the window disagree with where the
/// margin really is. It is deliberately *not* called unconditionally the
/// way gpui's own `window_shadow` example does it — that would announce a
/// margin under `Decorations::Server` that this function never draws.
pub(crate) fn wrap(content: impl IntoElement, theme: &Theme, window: &mut Window) -> AnyElement {
    let Decorations::Client { tiling } = window.window_decorations() else {
        return content.into_any_element();
    };
    window.set_client_inset(INSET);

    let layout = FrameLayout::for_tiling(tiling);

    div()
        .id("window-frame")
        .relative()
        .size_full()
        .child(
            div()
                .size_full()
                .when(layout.edge_top, |d| d.pt(INSET))
                .when(layout.edge_right, |d| d.pr(INSET))
                .when(layout.edge_bottom, |d| d.pb(INSET))
                .when(layout.edge_left, |d| d.pl(INSET))
                .child(
                    div()
                        .size_full()
                        .overflow_hidden()
                        .border_color(theme.border_strong)
                        .when(layout.edge_top, |d| d.border_t(BORDER))
                        .when(layout.edge_right, |d| d.border_r(BORDER))
                        .when(layout.edge_bottom, |d| d.border_b(BORDER))
                        .when(layout.edge_left, |d| d.border_l(BORDER))
                        .when(layout.corner_top_left, |d| d.rounded_tl(px(ui::RADIUS)))
                        .when(layout.corner_top_right, |d| d.rounded_tr(px(ui::RADIUS)))
                        .when(layout.corner_bottom_left, |d| d.rounded_bl(px(ui::RADIUS)))
                        .when(layout.corner_bottom_right, |d| d.rounded_br(px(ui::RADIUS)))
                        // A tiled window butts against the screen edge or a
                        // neighbor on at least one side, so it never floats
                        // — no shadow to cast, matching every desktop
                        // environment's own tiled-window treatment.
                        .when(!tiling.is_tiled(), |d| d.shadow_lg())
                        .child(content),
                ),
        )
        // Resize handles live in the margin the padding above just opened
        // up, positioned against the outer, unpadded edge of this element
        // so they never overlap the (inset) visible content.
        .when(layout.edge_top, |d| {
            d.child(horizontal_handle("resize-top", ResizeEdge::Top, true))
        })
        .when(layout.edge_bottom, |d| {
            d.child(horizontal_handle(
                "resize-bottom",
                ResizeEdge::Bottom,
                false,
            ))
        })
        .when(layout.edge_left, |d| {
            d.child(vertical_handle("resize-left", ResizeEdge::Left, true))
        })
        .when(layout.edge_right, |d| {
            d.child(vertical_handle("resize-right", ResizeEdge::Right, false))
        })
        .when(layout.corner_top_left, |d| {
            d.child(corner_handle(
                "resize-top-left",
                ResizeEdge::TopLeft,
                CursorStyle::ResizeUpLeftDownRight,
                true,
                true,
            ))
        })
        .when(layout.corner_top_right, |d| {
            d.child(corner_handle(
                "resize-top-right",
                ResizeEdge::TopRight,
                CursorStyle::ResizeUpRightDownLeft,
                true,
                false,
            ))
        })
        .when(layout.corner_bottom_left, |d| {
            d.child(corner_handle(
                "resize-bottom-left",
                ResizeEdge::BottomLeft,
                CursorStyle::ResizeUpRightDownLeft,
                false,
                true,
            ))
        })
        .when(layout.corner_bottom_right, |d| {
            d.child(corner_handle(
                "resize-bottom-right",
                ResizeEdge::BottomRight,
                CursorStyle::ResizeUpLeftDownRight,
                false,
                false,
            ))
        })
        .into_any_element()
}

/// A resize handle spanning the top or bottom edge, inset from the corners
/// (which have their own handles below) so the two never overlap.
fn horizontal_handle(id: &'static str, edge: ResizeEdge, at_top: bool) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .left(INSET)
        .right(INSET)
        .h(INSET)
        .cursor(CursorStyle::ResizeUpDown)
        .when(at_top, |d| d.top_0())
        .when(!at_top, |d| d.bottom_0())
        .on_mouse_down(MouseButton::Left, move |_, window, _| {
            window.start_window_resize(edge);
        })
}

/// A resize handle spanning the left or right edge, inset from the corners
/// the same way `horizontal_handle` is.
fn vertical_handle(id: &'static str, edge: ResizeEdge, at_left: bool) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .top(INSET)
        .bottom(INSET)
        .w(INSET)
        .cursor(CursorStyle::ResizeLeftRight)
        .when(at_left, |d| d.left_0())
        .when(!at_left, |d| d.right_0())
        .on_mouse_down(MouseButton::Left, move |_, window, _| {
            window.start_window_resize(edge);
        })
}

/// A square resize handle at one of the four corners.
fn corner_handle(
    id: &'static str,
    edge: ResizeEdge,
    cursor: CursorStyle,
    at_top: bool,
    at_left: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .w(INSET)
        .h(INSET)
        .cursor(cursor)
        .when(at_top, |d| d.top_0())
        .when(!at_top, |d| d.bottom_0())
        .when(at_left, |d| d.left_0())
        .when(!at_left, |d| d.right_0())
        .on_mouse_down(MouseButton::Left, move |_, window, _| {
            window.start_window_resize(edge);
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiling(top: bool, right: bool, bottom: bool, left: bool) -> Tiling {
        Tiling {
            top,
            right,
            bottom,
            left,
        }
    }

    #[test]
    fn untiled_window_rounds_every_corner_and_resizes_every_edge() {
        let layout = FrameLayout::for_tiling(Tiling::default());
        assert!(layout.edge_top && layout.edge_right && layout.edge_bottom && layout.edge_left);
        assert!(
            layout.corner_top_left
                && layout.corner_top_right
                && layout.corner_bottom_left
                && layout.corner_bottom_right
        );
    }

    #[test]
    fn fully_tiled_window_has_no_rounded_corners_or_resize_edges() {
        let layout = FrameLayout::for_tiling(Tiling::tiled());
        assert!(!layout.edge_top && !layout.edge_right && !layout.edge_bottom && !layout.edge_left);
        assert!(
            !layout.corner_top_left
                && !layout.corner_top_right
                && !layout.corner_bottom_left
                && !layout.corner_bottom_right
        );
    }

    #[test]
    fn snapped_left_keeps_only_the_shared_right_divider_free() {
        // A GNOME/KDE "snap left" tiles the window's top, bottom, and left
        // edges (screen edge or a neighboring tiled window on each); only
        // the right edge — the shared divider with whatever is snapped
        // beside it — stays free to resize, and no corner survives since
        // every corner touches at least one tiled edge.
        let layout = FrameLayout::for_tiling(tiling(true, false, true, true));
        assert!(!layout.edge_top);
        assert!(layout.edge_right);
        assert!(!layout.edge_bottom);
        assert!(!layout.edge_left);
        assert!(!layout.corner_top_left);
        assert!(!layout.corner_top_right);
        assert!(!layout.corner_bottom_left);
        assert!(!layout.corner_bottom_right);
    }

    #[test]
    fn top_edge_tiled_alone_squares_off_only_the_top_two_corners() {
        // Maximized-vertically-only is the shape a plain (non-fullscreen)
        // maximize tends to produce on X11 (see `window_decorations` in
        // gpui's X11 backend) — top and bottom tiled, left/right free.
        let layout = FrameLayout::for_tiling(tiling(true, false, false, false));
        assert!(layout.edge_left && layout.edge_right);
        assert!(!layout.corner_top_left && !layout.corner_top_right);
        assert!(layout.corner_bottom_left && layout.corner_bottom_right);
    }
}

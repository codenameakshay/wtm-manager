//! A reusable right-click context menu that any view can host. See
//! `crate::app::WtmApp`'s `context_menu` field for a real call site.
//!
//! The menu tracks no notion of what it was opened *for* — a host that
//! needs that back once the menu acts on a choice keeps its own copy
//! alongside the id it gets from `on_select`, set at the same time it
//! calls [`ContextMenu::open`], the way `crate::app`'s `context_menu_target`
//! does.
//!
//! `render` takes `Window`/`App` because GPUI only routes key events to the
//! currently *focused* element, so the menu has to briefly claim keyboard
//! focus while open, and `render` is the one call every host already makes
//! once per frame.
//!
//! The open/closed state lives behind `Rc<RefCell<..>>`, shared between the
//! `ContextMenu` the host owns and the listeners `render` hands to GPUI, so
//! the menu can close *itself* — on an outside click, on Escape, or after a
//! selection, always *before* calling `on_select` — and a host's
//! `on_select` can assume the menu is already gone.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    anchored, deferred, div, px, AnyElement, App, Corner, FocusHandle, KeyDownEvent, MouseButton,
    Pixels, Point, SharedString, Window,
};

use crate::motion;
use crate::theme::{Theme, RADIUS_CONTROL, ROW_HEIGHT, SPACE_2, SPACE_4, SPACE_8};
use crate::ui;

/// Renders above everything painted normally — see `gpui::deferred`. There is
/// only one overlay layer in this app today, so any value above the default
/// (0) is enough; picked well clear of it in case a modal joins it later.
const OVERLAY_PRIORITY: usize = 100;

/// Wide enough that a shortcut hint never collides with a label.
const MIN_WIDTH: f32 = 200.0;

/// The shape a host's selection callback takes, named so it does not have to
/// be spelled out (and re-triggers `clippy::type_complexity`) at every call
/// site that stores or forwards one.
type OnSelect = dyn Fn(&str, &mut Window, &mut App);

/// One row in a context menu: a selectable action, or a visual divider.
///
/// Separators carry no id or label — they exist only to be skipped, both
/// visually (rendered as a rule, not a row) and by keyboard navigation.
pub struct MenuItem {
    id: SharedString,
    label: SharedString,
    icon: Option<&'static str>,
    shortcut: Option<SharedString>,
    danger: bool,
    enabled: bool,
    is_separator: bool,
}

impl MenuItem {
    /// A selectable action, enabled by default.
    pub fn action(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            shortcut: None,
            danger: false,
            enabled: true,
            is_separator: false,
        }
    }

    /// A thin rule between groups of actions.
    pub fn separator() -> Self {
        Self {
            id: SharedString::default(),
            label: SharedString::default(),
            icon: None,
            shortcut: None,
            danger: false,
            enabled: false,
            is_separator: true,
        }
    }

    /// A leading icon, tinted `text_tertiary` (or `danger` on a danger item).
    pub fn icon(mut self, path: &'static str) -> Self {
        self.icon = Some(path);
        self
    }

    /// A trailing shortcut hint, e.g. "⌘⌫".
    pub fn shortcut(mut self, hint: impl Into<SharedString>) -> Self {
        self.shortcut = Some(hint.into());
        self
    }

    /// Marks the action as destructive: label and icon render in `danger`.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    /// Greys the row out and makes it inert: no hover wash, no click, no
    /// keyboard highlight.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Whether keyboard navigation may land here.
    fn is_selectable(&self) -> bool {
        !self.is_separator && self.enabled
    }
}

/// Everything that exists only while the menu is open, including the pieces
/// GPUI's `'static` listeners need to reach without a `&mut ContextMenu`.
struct OpenState {
    position: Point<Pixels>,
    items: Vec<MenuItem>,
    /// Keyboard highlight, an index into `items`. A bare `Cell` is enough —
    /// it is only ever reached through `OpenState`'s own `Ref`, which already
    /// grants shared access.
    highlighted: Cell<Option<usize>>,
    /// Created lazily on first render (see the module doc for why `open`
    /// cannot create it up front) and reused across renders so the menu does
    /// not steal focus from itself every frame.
    focus_handle: RefCell<Option<FocusHandle>>,
    /// Whatever held focus just before the menu claimed it, restored on
    /// every internal dismissal path.
    previous_focus: RefCell<Option<FocusHandle>>,
}

/// A small, reusable right-click menu. Owned by whichever view hosts it —
/// typically one field alongside the rest of that view's UI state.
pub struct ContextMenu {
    state: Rc<RefCell<Option<OpenState>>>,
}

impl ContextMenu {
    pub fn new() -> Self {
        Self {
            state: Rc::new(RefCell::new(None)),
        }
    }

    /// Open at `position` (window coordinates), replacing whatever the menu
    /// is currently showing. A host that needs to know what the menu was
    /// opened for once it acts on a choice keeps its own copy, as
    /// `crate::app`'s `context_menu_target` does.
    pub fn open(&mut self, position: Point<Pixels>, items: Vec<MenuItem>) {
        *self.state.borrow_mut() = Some(OpenState {
            position,
            items,
            highlighted: Cell::new(None),
            focus_handle: RefCell::new(None),
            previous_focus: RefCell::new(None),
        });
    }

    /// Close the menu without restoring focus — callers with a `Window` in
    /// hand (an outside click, Escape, a selection) go through the internal
    /// dismissal path instead, which does restore it. This method exists for
    /// a host that needs to force the menu shut from somewhere without one,
    /// e.g. because the row it was opened for just disappeared.
    pub fn close(&mut self) {
        *self.state.borrow_mut() = None;
    }

    pub fn is_open(&self) -> bool {
        self.state.borrow().is_some()
    }

    /// Render the overlay. Returns `None` when closed. `on_select` fires with
    /// the chosen item's id; the menu has already closed itself by the time
    /// it fires (see the module doc's dismissal convention).
    pub fn render(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut App,
        on_select: impl Fn(&str, &mut Window, &mut App) + 'static,
    ) -> Option<AnyElement> {
        if !self.is_open() {
            return None;
        }
        let focus_handle = self.claim_focus(window, cx)?;
        let on_select: Rc<OnSelect> = Rc::new(on_select);

        let (position, rows) = {
            let borrow = self.state.borrow();
            let open = borrow.as_ref()?;
            let highlighted = open.highlighted.get();
            let rows = open
                .items
                .iter()
                .enumerate()
                .map(|(ix, item)| {
                    if item.is_separator {
                        Self::render_separator(theme)
                    } else {
                        Self::render_item(
                            ix,
                            item,
                            highlighted == Some(ix),
                            theme,
                            &self.state,
                            &on_select,
                        )
                    }
                })
                .collect::<Vec<_>>();
            (open.position, rows)
        };

        // Sits *under* the menu panel in the same deferred pass: it covers
        // the window so any click outside the panel hits it first, closing
        // the menu, and `occlude()` stops that same click from also reaching
        // whatever real row sits beneath it.
        let catcher_state = self.state.clone();
        let catcher = div()
            .id("context-menu-catcher")
            .absolute()
            .inset_0()
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_event, window, _cx| {
                Self::dismiss(&catcher_state, window);
            });

        let key_state = self.state.clone();
        let key_on_select = on_select.clone();
        // `ui::popover`: the shared menu/palette/context-menu surface —
        // `RADIUS_PANEL`, `shadow_popover`, `surface_overlay`.
        let panel = ui::popover(theme)
            .id("context-menu-panel")
            .track_focus(&focus_handle)
            .on_key_down(move |event, window, cx| {
                Self::handle_key(&key_state, &key_on_select, event, window, cx);
            })
            .occlude()
            .min_w(px(MIN_WIDTH))
            .p(px(SPACE_4))
            .gap(px(SPACE_2))
            .children(rows);

        // The list beneath never animates (it's touched on every
        // scroll/refresh), but this menu is touched rarely, so it enters
        // with `MENU_IN` — the motion is what tells the eye where it came
        // from.
        let panel = motion::menu_in("context-menu-panel-motion", panel, cx);

        Some(
            deferred(
                div().absolute().inset_0().child(catcher).child(
                    anchored()
                        .position(position)
                        .anchor(Corner::TopLeft)
                        .snap_to_window_with_margin(px(SPACE_8))
                        .child(panel),
                ),
            )
            .with_priority(OVERLAY_PRIORITY)
            .into_any_element(),
        )
    }

    /// Claim keyboard focus for the menu's own `on_key_down`, creating and
    /// caching the `FocusHandle` on first call and remembering whatever was
    /// focused beforehand so it can be restored on dismissal.
    fn claim_focus(&self, window: &mut Window, cx: &mut App) -> Option<FocusHandle> {
        let borrow = self.state.borrow();
        let open = borrow.as_ref()?;
        let mut handle_slot = open.focus_handle.borrow_mut();
        if handle_slot.is_none() {
            *open.previous_focus.borrow_mut() = window.focused(cx);
            *handle_slot = Some(cx.focus_handle());
        }
        let handle = handle_slot.clone()?;
        drop(handle_slot);
        drop(borrow);
        if !handle.is_focused(window) {
            window.focus(&handle);
        }
        Some(handle)
    }

    /// Ends the menu's life: clears the shared state and restores whatever
    /// focus preceded it. Every internal dismissal path — outside click,
    /// Escape, selection — goes through this, which is what lets the menu
    /// close itself without the host's cooperation.
    fn dismiss(state: &Rc<RefCell<Option<OpenState>>>, window: &mut Window) {
        let old = state.borrow_mut().take();
        if let Some(old) = old {
            if let Some(previous) = old.previous_focus.into_inner() {
                window.focus(&previous);
            }
        }
        window.refresh();
    }

    /// Looks `id` up among `items` and hands it back only if it names an
    /// enabled action — the single gate both the mouse and keyboard paths go
    /// through, so a disabled item can never reach `on_select`.
    fn selectable_id<'a>(items: &'a [MenuItem], id: &str) -> Option<&'a SharedString> {
        items
            .iter()
            .find(|item| !item.is_separator && item.id.as_ref() == id)
            .filter(|item| item.enabled)
            .map(|item| &item.id)
    }

    fn select(
        state: &Rc<RefCell<Option<OpenState>>>,
        on_select: &Rc<OnSelect>,
        id: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        let allowed = state
            .borrow()
            .as_ref()
            .is_some_and(|open| Self::selectable_id(&open.items, id).is_some());
        if !allowed {
            return;
        }
        Self::dismiss(state, window);
        on_select(id, window, cx);
    }

    fn handle_key(
        state: &Rc<RefCell<Option<OpenState>>>,
        on_select: &Rc<OnSelect>,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut App,
    ) {
        match event.keystroke.key.as_str() {
            "escape" => Self::dismiss(state, window),
            "down" => Self::move_highlight(state, 1, window),
            "up" => Self::move_highlight(state, -1, window),
            "enter" => Self::activate_highlighted(state, on_select, window, cx),
            _ => {}
        }
    }

    fn move_highlight(state: &Rc<RefCell<Option<OpenState>>>, dir: i32, window: &mut Window) {
        let borrow = state.borrow();
        let Some(open) = borrow.as_ref() else {
            return;
        };
        let next = Self::next_selectable(&open.items, open.highlighted.get(), dir);
        open.highlighted.set(next);
        drop(borrow);
        // No entity to `cx.notify()` through — mark the window dirty
        // directly so the highlight's new position actually paints.
        window.refresh();
    }

    fn activate_highlighted(
        state: &Rc<RefCell<Option<OpenState>>>,
        on_select: &Rc<OnSelect>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let id = {
            let borrow = state.borrow();
            let Some(open) = borrow.as_ref() else {
                return;
            };
            let Some(ix) = open.highlighted.get() else {
                return;
            };
            let Some(item) = open.items.get(ix) else {
                return;
            };
            if !item.is_selectable() {
                return;
            }
            item.id.clone()
        };
        Self::dismiss(state, window);
        on_select(id.as_ref(), window, cx);
    }

    /// The next selectable (enabled, non-separator) index after `current` in
    /// direction `dir` (`1` for down, `-1` for up), wrapping around and
    /// skipping separators and disabled items. `None` if nothing qualifies.
    fn next_selectable(items: &[MenuItem], current: Option<usize>, dir: i32) -> Option<usize> {
        let len = items.len();
        if len == 0 {
            return None;
        }
        let start = current
            .map(|ix| ix as i64)
            .unwrap_or(if dir >= 0 { -1 } else { len as i64 });
        let mut cursor = start;
        for _ in 0..len {
            cursor = (cursor + dir as i64).rem_euclid(len as i64);
            if items[cursor as usize].is_selectable() {
                return Some(cursor as usize);
            }
        }
        None
    }

    /// A hairline rule with `SPACE_4` clearance above and below — reuses
    /// [`ui::divider`] rather than hand-rolling the same `theme.border`
    /// fill a second time.
    fn render_separator(theme: &Theme) -> AnyElement {
        ui::divider(theme).my(px(SPACE_4)).into_any_element()
    }

    fn render_item(
        ix: usize,
        item: &MenuItem,
        highlighted: bool,
        theme: &Theme,
        state: &Rc<RefCell<Option<OpenState>>>,
        on_select: &Rc<OnSelect>,
    ) -> AnyElement {
        let label_color = if !item.enabled {
            theme.text_ghost
        } else if item.danger {
            theme.danger
        } else {
            theme.text
        };
        let icon_color = if !item.enabled {
            theme.text_ghost
        } else if item.danger {
            theme.danger
        } else {
            theme.text_faint
        };

        // `RADIUS_CONTROL`: concentric with the popover's own `RADIUS_PANEL`
        // (10) at `SPACE_4` (4) padding — `10 - 4 == 6 == RADIUS_CONTROL`
        // exactly. `ROW_HEIGHT` so items line up with every other row.
        let mut row = div()
            .id(("context-menu-item", ix))
            .h(px(ROW_HEIGHT))
            .w_full()
            .px(px(SPACE_8))
            .flex()
            .items_center()
            .gap(px(SPACE_8))
            .rounded(px(RADIUS_CONTROL))
            .cursor_default()
            .when(highlighted && item.enabled, |this| {
                this.bg(theme.element_hover)
            });

        // Disabled items get no listener at all, not just a disabled-looking
        // one — the same structural guarantee `selectable_id` gives the
        // keyboard path.
        if item.enabled {
            let state = state.clone();
            let on_select = on_select.clone();
            let id = item.id.clone();
            row = row.hover(|this| this.bg(theme.element_hover)).on_click(
                move |_event, window, cx| {
                    Self::select(&state, &on_select, id.as_ref(), window, cx);
                },
            );
        }

        row.when_some(item.icon, |this, path| {
            this.child(ui::icon(path, 13.0, icon_color))
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(ui::TEXT_BASE))
                .text_color(label_color)
                .child(item.label.clone()),
        )
        .when_some(item.shortcut.clone(), |this, hint| {
            this.child(ui::kbd(&hint, theme))
        })
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui::point;

    use super::*;

    fn sample_items() -> Vec<MenuItem> {
        vec![
            MenuItem::action("open", "Open"),
            MenuItem::action("rename", "Rename").disabled(),
            MenuItem::separator(),
            MenuItem::action("delete", "Delete").danger(),
        ]
    }

    #[test]
    fn open_marks_menu_open() {
        let mut menu = ContextMenu::new();
        let position = point(px(10.0), px(20.0));
        menu.open(position, sample_items());

        assert!(menu.is_open());
    }

    #[test]
    fn close_clears_state() {
        let mut menu = ContextMenu::new();
        menu.open(point(px(0.0), px(0.0)), sample_items());

        menu.close();

        assert!(!menu.is_open());
    }

    #[test]
    fn selecting_a_disabled_item_is_rejected() {
        let items = sample_items();

        assert!(ContextMenu::selectable_id(&items, "rename").is_none());
        assert!(ContextMenu::selectable_id(&items, "open").is_some());
        assert!(ContextMenu::selectable_id(&items, "no-such-id").is_none());
    }

    #[test]
    fn keyboard_navigation_skips_separators_and_disabled() {
        let items = sample_items();
        // items: [0] open (selectable), [1] rename (disabled), [2] separator,
        // [3] delete (selectable).

        // From nothing highlighted, down lands on the first selectable row.
        assert_eq!(ContextMenu::next_selectable(&items, None, 1), Some(0));

        // From "open", down skips the disabled row and the separator.
        assert_eq!(ContextMenu::next_selectable(&items, Some(0), 1), Some(3));

        // Wraps back around to "open".
        assert_eq!(ContextMenu::next_selectable(&items, Some(3), 1), Some(0));

        // Moving up from "open" wraps to "delete", again skipping over the
        // separator and the disabled row.
        assert_eq!(ContextMenu::next_selectable(&items, Some(0), -1), Some(3));
    }

    #[test]
    fn keyboard_navigation_with_nothing_selectable_stays_none() {
        let items = vec![MenuItem::action("x", "X").disabled(), MenuItem::separator()];
        assert_eq!(ContextMenu::next_selectable(&items, None, 1), None);
    }
}

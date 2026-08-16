//! A reusable right-click context menu that any view can host.
//!
//! # Why generic over the target
//!
//! [`ContextMenu<T>`] is generic over whatever the menu was opened *for* (a
//! row index, a `PathBuf`, an enum of hoverable things — the host decides)
//! rather than pinning that down to a `SharedString` id, so [`ContextMenu::open`]
//! type-checks against whatever a given host actually opens the menu for.
//!
//! `T` is *not* retained, though — [`open`](ContextMenu::open) takes a
//! `target: T` and drops it. This module used to hand it back through a
//! `ContextMenu::target` accessor, but every dismissal path (outside click,
//! Escape, choosing an item) closes the menu — and, on the selection paths,
//! calls the host's `on_select` — while sharing one `Rc<RefCell<..>>` for
//! state; making a stored target outlive dismissal so `on_select` could read
//! it back would mean either changing `on_select`'s signature to carry it
//! directly, or threading dismissal through two phases so `is_open()` and
//! the stored target disagree about whether the menu is still open for the
//! callback's duration. Neither is worth it for a value nothing here reads:
//! the one host this module has ([`crate::app`]) already keeps its own copy
//! (`context_menu_target`) alongside the id it gets from `on_select`, set at
//! the same time it calls `open`. A host with the same need should do the
//! same. `T` stays as a type parameter purely so `open`'s caller still gets
//! a compile-time check that they are handing this menu instance the kind of
//! target it was built for.
//!
//! # Why `render` takes `Window` and `App`
//!
//! GPUI only routes key events to the currently *focused* element and its
//! ancestors, not to arbitrary elements painted on top of everything else.
//! For up/down/enter/escape to reach the menu at all, the menu has to briefly
//! own keyboard focus while it is open. Claiming a `FocusHandle` requires
//! `cx.focus_handle()` and `window.focus(..)`, and — since [`ContextMenu::open`]
//! deliberately does not take a `Window`/`App` (a right-click handler may not
//! have one handy either) — the only place left to do it is `render`, the
//! one call every host already makes once per frame. This is the one place
//! this module's API departs from a bare "label/id" sketch: it earns its
//! keep by making keyboard navigation real instead of mouse-only.
//!
//! # Why interior mutability
//!
//! Every listener this module attaches (`on_click`, `on_mouse_down`,
//! `on_key_down`) must be `'static` and cannot borrow `&mut ContextMenu`
//! across the time between "the menu was painted" and "the user did
//! something to it". So the open/closed state lives behind `Rc<RefCell<..>>`, shared
//! between the `ContextMenu` the host owns and the listeners `render` hands
//! to GPUI. This is what lets the menu close *itself* — on an outside click,
//! on Escape, or after a selection — without the host having to remember to
//! call [`ContextMenu::close`] in response.
//!
//! # Dismissal convention
//!
//! For every path that ends the menu's life (outside click, Escape, or
//! choosing an item), the menu closes itself *before* calling `on_select`.
//! A host's `on_select` can therefore assume the menu is already gone —
//! including if it decides to open a new one for a different target.
//!
//! # Example
//!
//! ```ignore
//! struct MyView {
//!     menu: ContextMenu<usize>,
//!     // What `menu` is currently open for. Kept alongside it — see "Why
//!     // generic over the target" above for why the menu itself does not
//!     // hand this back.
//!     menu_target: Option<usize>,
//! }
//!
//! // On right-click:
//! self.menu_target = Some(row_ix);
//! menu.open(row_ix, event.position, vec![
//!     MenuItem::action("open", "Open in Editor").icon(icons::OPEN_EXTERNAL),
//!     MenuItem::separator(),
//!     MenuItem::action("delete", "Delete Worktree").icon(icons::TRASH).danger(),
//! ]);
//!
//! // In render:
//! .children(self.menu.render(&theme, window, cx, cx.listener(|this, id: &str, _window, cx| {
//!     if let Some(row_ix) = this.menu_target.take() {
//!         this.handle_menu_action(row_ix, id, cx);
//!     }
//! })))
//! ```

use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    anchored, deferred, div, px, AnyElement, App, Corner, FocusHandle, KeyDownEvent, MouseButton,
    Pixels, Point, SharedString, Window,
};

use crate::theme::Theme;
use crate::ui;

/// Renders above everything painted normally — see `gpui::deferred`. There is
/// only one overlay layer in this app today, so any value above the default
/// (0) is enough; picked well clear of it in case a modal joins it later.
const OVERLAY_PRIORITY: usize = 100;

/// Wide enough that a shortcut hint never collides with a label.
const MIN_WIDTH: f32 = 200.0;
const ITEM_HEIGHT: f32 = 26.0;
const ITEM_RADIUS: f32 = 6.0;
const PANEL_RADIUS: f32 = 8.0;

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
///
/// Not generic over `T`: see the module doc's "Why generic over the target"
/// section for why the target itself is not part of this state.
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
pub struct ContextMenu<T: 'static> {
    state: Rc<RefCell<Option<OpenState>>>,
    /// `open` accepts a `target: T` purely so a caller cannot hand this menu
    /// instance a target of the wrong type; the value itself is dropped
    /// rather than stored (see the module doc). This marker is what lets the
    /// struct stay generic over `T` — and so keep that compile-time check —
    /// without an `OpenState` field of type `T` to hold a value that is
    /// never actually kept.
    _target: PhantomData<T>,
}

impl<T: 'static> Default for ContextMenu<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: 'static> ContextMenu<T> {
    pub fn new() -> Self {
        Self {
            state: Rc::new(RefCell::new(None)),
            _target: PhantomData,
        }
    }

    /// Open at `position` (window coordinates) for `target`, replacing
    /// whatever the menu was previously showing. `target` is not retained —
    /// see the module doc's "Why generic over the target" section — so a
    /// host that needs it back once the menu acts on a choice must keep its
    /// own copy, as `crate::app`'s `context_menu_target` does.
    pub fn open(&mut self, _target: T, position: Point<Pixels>, items: Vec<MenuItem>) {
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
        let panel = div()
            .id("context-menu-panel")
            .track_focus(&focus_handle)
            .on_key_down(move |event, window, cx| {
                Self::handle_key(&key_state, &key_on_select, event, window, cx);
            })
            .occlude()
            .min_w(px(MIN_WIDTH))
            .p(px(4.0))
            .flex()
            .flex_col()
            .gap(px(1.0))
            .rounded(px(PANEL_RADIUS))
            .bg(theme.raised)
            .border_1()
            .border_color(theme.border_strong)
            .shadow_lg()
            .children(rows);

        Some(
            deferred(
                div().absolute().inset_0().child(catcher).child(
                    anchored()
                        .position(position)
                        .anchor(Corner::TopLeft)
                        .snap_to_window_with_margin(px(8.0))
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

    fn render_separator(theme: &Theme) -> AnyElement {
        div()
            .h(px(1.0))
            .my(px(4.0))
            .bg(theme.border)
            .into_any_element()
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
            theme.text_tertiary
        };

        let mut row = div()
            .id(("context-menu-item", ix))
            .h(px(ITEM_HEIGHT))
            .w_full()
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(px(ITEM_RADIUS))
            .cursor_default()
            .when(highlighted && item.enabled, |this| this.bg(theme.item_wash));

        // Disabled items get no listener at all, not just a disabled-looking
        // one — the same structural guarantee `selectable_id` gives the
        // keyboard path.
        if item.enabled {
            let state = state.clone();
            let on_select = on_select.clone();
            let id = item.id.clone();
            row = row
                .hover(|this| this.bg(theme.item_wash))
                .on_click(move |_event, window, cx| {
                    Self::select(&state, &on_select, id.as_ref(), window, cx);
                });
        }

        row.when_some(item.icon, |this, path| {
            this.child(ui::icon(path, 13.0, icon_color))
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(12.5))
                .text_color(label_color)
                .child(item.label.clone()),
        )
        .when_some(item.shortcut.clone(), |this, hint| {
            this.child(
                div()
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(theme.text_ghost)
                    .child(hint),
            )
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
        menu.open(42usize, position, sample_items());

        assert!(menu.is_open());
    }

    #[test]
    fn close_clears_state() {
        let mut menu = ContextMenu::new();
        menu.open(1usize, point(px(0.0), px(0.0)), sample_items());

        menu.close();

        assert!(!menu.is_open());
    }

    #[test]
    fn is_open_reflects_state() {
        let mut menu: ContextMenu<usize> = ContextMenu::new();
        assert!(!menu.is_open());

        menu.open(1, point(px(0.0), px(0.0)), sample_items());
        assert!(menu.is_open());

        menu.close();
        assert!(!menu.is_open());
    }

    #[test]
    fn selecting_a_disabled_item_is_rejected() {
        let items = sample_items();

        assert!(ContextMenu::<usize>::selectable_id(&items, "rename").is_none());
        assert!(ContextMenu::<usize>::selectable_id(&items, "open").is_some());
        assert!(ContextMenu::<usize>::selectable_id(&items, "no-such-id").is_none());
    }

    #[test]
    fn keyboard_navigation_skips_separators_and_disabled() {
        let items = sample_items();
        // items: [0] open (selectable), [1] rename (disabled), [2] separator,
        // [3] delete (selectable).

        // From nothing highlighted, down lands on the first selectable row.
        assert_eq!(
            ContextMenu::<usize>::next_selectable(&items, None, 1),
            Some(0)
        );

        // From "open", down skips the disabled row and the separator.
        assert_eq!(
            ContextMenu::<usize>::next_selectable(&items, Some(0), 1),
            Some(3)
        );

        // Wraps back around to "open".
        assert_eq!(
            ContextMenu::<usize>::next_selectable(&items, Some(3), 1),
            Some(0)
        );

        // Moving up from "open" wraps to "delete", again skipping over the
        // separator and the disabled row.
        assert_eq!(
            ContextMenu::<usize>::next_selectable(&items, Some(0), -1),
            Some(3)
        );
    }

    #[test]
    fn keyboard_navigation_with_nothing_selectable_stays_none() {
        let items = vec![MenuItem::action("x", "X").disabled(), MenuItem::separator()];
        assert_eq!(ContextMenu::<usize>::next_selectable(&items, None, 1), None);
    }

    #[test]
    fn menu_item_builders_set_fields() {
        let item = MenuItem::action("id", "Label")
            .icon("path")
            .shortcut("⌘K")
            .danger();

        assert_eq!(item.id.as_ref(), "id");
        assert_eq!(item.label.as_ref(), "Label");
        assert_eq!(item.icon, Some("path"));
        assert_eq!(item.shortcut.as_ref().map(SharedString::as_ref), Some("⌘K"));
        assert!(item.danger);
        assert!(item.enabled);
        assert!(item.is_selectable());

        let disabled = MenuItem::action("x", "X").disabled();
        assert!(!disabled.enabled);
        assert!(!disabled.is_selectable());

        let separator = MenuItem::separator();
        assert!(separator.is_separator);
        assert!(!separator.is_selectable());
    }
}

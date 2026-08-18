//! A single-line text field.
//!
//! GPUI ships no editable text widget of its own, so this implements
//! [`EntityInputHandler`] directly — the interface every platform's IME,
//! dictation, and Unicode input palette route composed text through. The
//! implementation is adapted from gpui's own `examples/input.rs`
//! (Apache-2.0 licensed), trimmed to single line only: that is all a search
//! box, a rename field, or a new-worktree name ever needs, and dropping
//! wrapping keeps the layout math (and this file) small enough to read in
//! one sitting.
//!
//! Two things the original example did not need, added here: painting from
//! [`Theme::of`] instead of hard-coded colors, and an [`InputEvent`] stream
//! so a parent dialog can react to Enter/Escape without owning key dispatch
//! itself. One thing dropped: the character-palette action — this app has
//! no use for it, and every action left in `text_input` below binds a key a
//! dialog actually needs.
//!
//! # Paint only, per the redesign
//!
//! The redesign (SURFACES §10) touches only how this file paints: the field
//! is an inset well (`theme.surface_inset`), focus adds a stronger edge (see
//! [`TextInput::render`]'s use of [`crate::ui::focus_ring`]), the
//! placeholder reads at `text_faint`, and the caret/selection band paint
//! from `Theme`'s dedicated `caret`/`selection` tokens (see their docs on
//! `Theme` for why those exist rather than a bare `theme.accent`).
//! [`TextInput::borderless`] is the one new, purely-additive knob: an
//! overlay that supplies its own containing well
//! (the command palette's search field, sitting inside `ui::popover`) can
//! opt out of this file's own background/border so it doesn't nest a second
//! box around the first. Every IME/selection/keyboard code path below is
//! unchanged.
//!
//! The original steps the cursor by grapheme cluster via the
//! `unicode-segmentation` crate. This crate may only touch its own two
//! files, so `unicode-segmentation` is not a dependency here — cursor
//! movement below steps by Unicode scalar value (`char`) instead. The two
//! only disagree inside a multi-codepoint grapheme cluster (an emoji with a
//! skin-tone modifier, a combining accent typed as two codepoints); every
//! string this app collects — worktree and branch names — is plain
//! ASCII/Latin in practice, so the simplification is invisible in use.

use std::ops::Range;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    actions, div, fill, point, px, size, App, Bounds, ClipboardItem, Context, CursorStyle,
    ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, TextRun,
    UTF16Selection, UnderlineStyle, Window,
};

use crate::theme::{Theme, RADIUS_CONTROL, SPACE_6, SPACE_8};
use crate::ui;

/// How long the caret stays in each phase of its blink. Matches the ~530ms
/// most desktop text fields use — fast enough that the caret doesn't read
/// as "gone", slow enough not to flicker.
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        Submit,
        Cancel,
    ]
);

/// What a parent needs to know about without owning key dispatch itself —
/// a dialog listens for `Submit`/`Cancel` instead of binding Enter/Escape
/// on top of whatever the field is already doing with them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputEvent {
    /// Enter was pressed.
    Submit,
    /// Escape was pressed.
    Cancel,
    /// The content changed: typing, IME composition, paste, cut, or an
    /// edit action (backspace/delete). `set_value` does not emit this — a
    /// programmatic reset is not something the user did.
    Changed,
}

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    /// Flipped on an interval by a background task to blink the caret.
    /// Only consulted while painting a focused field with no selection —
    /// with a selection there is no caret quad, blinking or otherwise.
    cursor_visible: bool,
    /// When set, [`TextInput::render`] skips its own background/border
    /// entirely — see [`TextInput::borderless`]'s doc.
    borderless: bool,
}

impl TextInput {
    pub fn new(
        placeholder: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::ensure_keymap(cx);
        Self::start_blinking(cx);

        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            cursor_visible: true,
            borderless: false,
        }
    }

    /// Opt out of this field's own background/border/radius paint. Builder
    /// style so it chains straight onto [`TextInput::new`], before the
    /// value is wrapped in an `Entity` (e.g.
    /// `cx.new(|cx| TextInput::new(placeholder, window, cx).borderless())`).
    ///
    /// For every existing field (dialogs, the filter box, the run panel's
    /// command field) the default — a bordered inset well — is exactly
    /// right, so this is opt-in and changes nothing for them. It exists for
    /// the one caller that already supplies its own containing well: the
    /// command palette's search field sits inside `ui::popover` and wants
    /// to blend into it (SURFACES §6: "a borderless inset well") rather
    /// than draw a second, redundant box nested inside the first. Purely a
    /// paint switch — [`TextInput::render`] is the only reader of this
    /// field, and every IME/selection/keyboard path elsewhere in this file
    /// is unaffected either way.
    pub fn borderless(mut self) -> Self {
        self.borderless = true;
        self
    }

    pub fn value(&self) -> &str {
        &self.content
    }

    /// Replace the content wholesale — used to preload a dialog field or to
    /// clear it after a submit. Does not emit [`InputEvent::Changed`]; see
    /// that variant's doc comment for why.
    pub fn set_value(
        &mut self,
        text: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.content = text.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    /// Register this module's keymap once, no matter how many `TextInput`s
    /// exist — every field shares one static Backspace/Left/Right/… action
    /// set, so there is nothing per-instance to bind. Scoped to the
    /// `"TextInput"` key context so it only fires while a field is focused,
    /// leaving the app's own Enter/arrow bindings alone everywhere else.
    fn ensure_keymap(cx: &mut Context<Self>) {
        if cx.has_global::<KeymapRegistered>() {
            return;
        }
        cx.set_global(KeymapRegistered);
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, Some("TextInput")),
            KeyBinding::new("delete", Delete, Some("TextInput")),
            KeyBinding::new("left", Left, Some("TextInput")),
            KeyBinding::new("right", Right, Some("TextInput")),
            KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
            KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
            KeyBinding::new("cmd-a", SelectAll, Some("TextInput")),
            KeyBinding::new("cmd-c", Copy, Some("TextInput")),
            KeyBinding::new("cmd-v", Paste, Some("TextInput")),
            KeyBinding::new("cmd-x", Cut, Some("TextInput")),
            KeyBinding::new("home", Home, Some("TextInput")),
            KeyBinding::new("end", End, Some("TextInput")),
            // The macOS convention for "line start/end" is cmd+left/right;
            // there is only one line here, so it is identical to Home/End.
            KeyBinding::new("cmd-left", Home, Some("TextInput")),
            KeyBinding::new("cmd-right", End, Some("TextInput")),
            KeyBinding::new("enter", Submit, Some("TextInput")),
            KeyBinding::new("escape", Cancel, Some("TextInput")),
        ]);
    }

    /// Loop forever, flipping `cursor_visible` on an interval, until the
    /// entity is dropped (at which point `this.update` fails and the loop
    /// exits instead of leaking a timer per field that has ever existed).
    fn start_blinking(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            // `cx.background_executor().timer(..)`, not `gpui::Timer`/
            // `smol::Timer::after`: the latter is a raw wall-clock timer
            // that bypasses gpui's platform dispatcher, so nothing in a
            // `#[gpui::test]`'s `TestDispatcher` — including
            // `cx.executor().advance_clock(..)` — can see or control it.
            // See `crate::app::dialog_actions::submit_create_dialog`'s
            // matching fix for the full explanation (found via a real,
            // reproducible test failure caused by this exact pattern).
            cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
            let alive = this
                .update(cx, |this, cx| {
                    this.cursor_visible = !this.cursor_visible;
                    cx.notify();
                })
                .is_ok();
            if !alive {
                break;
            }
        })
        .detach();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn submit(&mut self, _: &Submit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Submit);
    }

    fn cancel(&mut self, _: &Cancel, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Cancel);
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // Single line only: a pasted newline would otherwise split the
            // field's content across a line break it can never render.
            self.replace_text_in_range(None, &text.replace('\n', " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        // A movement always shows the caret immediately, rather than
        // risking the keystroke landing during the blink's invisible half.
        self.cursor_visible = true;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.cursor_visible = true;
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        utf8_offset_from_utf16(&self.content, offset)
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        utf16_offset_from_utf8(&self.content, offset)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        previous_char_boundary(&self.content, offset)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        next_char_boundary(&self.content, offset)
    }
}

/// Byte offset of the character boundary just before `offset`, or `0` if
/// `offset` is already at (or before) the start.
fn previous_char_boundary(content: &str, offset: usize) -> usize {
    content
        .char_indices()
        .rev()
        .find_map(|(idx, _)| (idx < offset).then_some(idx))
        .unwrap_or(0)
}

/// Byte offset of the character boundary just after `offset`, or the
/// content's length if `offset` is already at (or past) the end.
fn next_char_boundary(content: &str, offset: usize) -> usize {
    content
        .char_indices()
        .find_map(|(idx, _)| (idx > offset).then_some(idx))
        .unwrap_or(content.len())
}

/// Convert a UTF-16 code-unit offset (what the system input APIs speak) to
/// a UTF-8 byte offset (what `content` is indexed by).
fn utf8_offset_from_utf16(content: &str, utf16_offset: usize) -> usize {
    let mut utf8_offset = 0;
    let mut utf16_count = 0;

    for ch in content.chars() {
        if utf16_count >= utf16_offset {
            break;
        }
        utf16_count += ch.len_utf16();
        utf8_offset += ch.len_utf8();
    }

    utf8_offset
}

/// Convert a UTF-8 byte offset to a UTF-16 code-unit offset.
fn utf16_offset_from_utf8(content: &str, utf8_offset: usize) -> usize {
    let mut utf16_offset = 0;
    let mut utf8_count = 0;

    for ch in content.chars() {
        if utf8_count >= utf8_offset {
            break;
        }
        utf8_count += ch.len_utf8();
        utf16_offset += ch.len_utf16();
    }

    utf16_offset
}

/// Marker global so [`TextInput::ensure_keymap`] binds its keys exactly
/// once per process.
struct KeymapRegistered;

impl gpui::Global for KeymapRegistered {}

impl EventEmitter<InputEvent> for TextInput {}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        self.cursor_visible = true;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        self.cursor_visible = true;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;

        assert_eq!(last_layout.text, self.content);
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

/// The element that actually shapes and paints the field's text, caret, and
/// selection highlight. Everything in `TextInput` above only manages state
/// and dispatches actions — painting needs the lower-level `Element` trait
/// because a caret and a selection band are geometry gpui's text layout
/// alone can't produce.
struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let theme = Theme::of(cx);
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let cursor_visible = input.cursor_visible;
        let style = window.text_style();

        // SURFACES §10: "Placeholder is text_faint."
        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), theme.text_faint)
        } else {
            (content, theme.text)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        // `theme.caret`/`theme.selection`: dedicated tokens for exactly
        // this — see their docs on `Theme` for why they exist (a caret and
        // a selection band are the "focus" half of accent's SPEC §3
        // mandate, tuned per appearance so `theme.text` painted on top
        // stays readable, rather than a bare `theme.accent` reused as-is).
        let cursor_pos = line.x_for_index(cursor);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                cursor_visible.then(|| {
                    fill(
                        Bounds::new(
                            point(bounds.left() + cursor_pos, bounds.top()),
                            size(px(2.), bounds.bottom() - bounds.top()),
                        ),
                        theme.caret,
                    )
                }),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    theme.selection,
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection)
        }
        let line = prepaint.line.take().unwrap();
        line.paint(bounds.origin, window.line_height(), window, cx)
            .unwrap();

        // Not a let-chain: this crate targets edition 2021, where those are
        // unstable.
        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let focused = self.focus_handle.is_focused(window);

        let field = div()
            .key_context("TextInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::cancel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .text_size(px(ui::TEXT_BASE))
            .line_height(px(18.0));

        // SURFACES §10: "the field itself is an inset well; focus adds
        // border_strong and the ring." `ui::focus_ring` — the design
        // system's one implementation of "the ring" (a real border, since
        // gpui has no inset box-shadow) — already paints a 2px accent edge
        // on focus, which reads strictly stronger than the resting 1px
        // `border` hairline; a separate `border_strong` layer underneath it
        // would never be visible, so this applies the ring directly rather
        // than painting a border nothing would show. Skipped in
        // `borderless` mode — see [`TextInput::borderless`]'s doc.
        let field = if self.borderless {
            field
        } else {
            field
                .px(px(SPACE_8))
                .py(px(SPACE_6))
                .rounded(px(RADIUS_CONTROL))
                .bg(theme.surface_inset)
                .border_1()
                .border_color(theme.border)
                .when(focused, ui::focus_ring(&theme))
        };

        field.child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_boundaries_step_over_ascii() {
        assert_eq!(previous_char_boundary("abc", 3), 2);
        assert_eq!(previous_char_boundary("abc", 1), 0);
        assert_eq!(previous_char_boundary("abc", 0), 0);
        assert_eq!(next_char_boundary("abc", 0), 1);
        assert_eq!(next_char_boundary("abc", 2), 3);
        assert_eq!(next_char_boundary("abc", 3), 3);
    }

    #[test]
    fn char_boundaries_step_over_multibyte_scalars() {
        // 'é' (U+00E9) is 2 bytes in UTF-8; boundaries must land on either
        // side of it, never inside.
        let s = "a\u{e9}bc";
        assert_eq!(next_char_boundary(s, 1), 3);
        assert_eq!(previous_char_boundary(s, 3), 1);
    }

    #[test]
    fn utf16_roundtrip_for_ascii() {
        let s = "hello";
        for i in 0..=s.len() {
            let utf16 = utf16_offset_from_utf8(s, i);
            assert_eq!(utf8_offset_from_utf16(s, utf16), i);
        }
    }

    #[test]
    fn utf16_offset_accounts_for_surrogate_pairs() {
        // U+1F600 is 4 bytes in UTF-8 but a surrogate pair (2 code units)
        // in UTF-16 — the two encodings disagree about "one character in",
        // which is exactly what these conversions exist to reconcile.
        let s = "a\u{1F600}b";
        assert_eq!(utf16_offset_from_utf8(s, 0), 0);
        assert_eq!(utf16_offset_from_utf8(s, 1), 1);
        assert_eq!(utf16_offset_from_utf8(s, 5), 3);
        assert_eq!(utf8_offset_from_utf16(s, 3), 5);
        assert_eq!(utf8_offset_from_utf16(s, 1), 1);
    }
}

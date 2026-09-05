//! Single- and multi-row selection, and the type-to-filter field: which
//! rows are visible under the current filter, which are selected, and the
//! keyboard/mouse handlers (arrow keys, shift-click, ⌘-click) that change
//! either.
//!
//! This module does not decide *what happens* to a selection (opening it,
//! removing it — see `commands` and `dialog_actions`); it only owns the
//! selection and filter state itself and keeps it consistent as the
//! underlying row set changes.

use super::*;

impl WtmApp {
    // -------------------------------------------------------------
    // Type-to-filter
    // -------------------------------------------------------------

    /// Indices into `self.rows` that pass the current filter, in original
    /// row order — a filter narrows *which* rows show, it does not
    /// re-rank them the way the palette's results do; the list's own
    /// "main first" ordering is part of what makes it scannable. An empty
    /// (or all-whitespace) query is every row, cheaply, without scoring.
    pub(super) fn visible_row_indices(&self, cx: &App) -> Vec<usize> {
        let value = self.filter_input.read(cx).value();
        let query = value.trim();
        if query.is_empty() {
            return (0..self.rows.len()).collect();
        }
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| palette::fuzzy_match(query, row.display_name()).is_some())
            .map(|(ix, _)| ix)
            .collect()
    }

    /// Re-establish "selection points at something visible" after the
    /// filter (or the row set itself) changes: drops any multi-selected
    /// rows that just became hidden, collapses a multi-selection that is
    /// left with one row or none, and snaps `selected` to the nearest
    /// visible row if it was filtered out from under it. Called after
    /// every filter keystroke and at the end of `apply_rows`, which is
    /// what keeps ↑/↓ (see `on_select_next`/`on_select_prev`) from ever
    /// having to special-case a stale selection themselves.
    pub(super) fn clamp_selection_to_filter(&mut self, cx: &mut Context<Self>) {
        let visible = self.visible_row_indices(cx);
        let visible_set: BTreeSet<usize> = visible.iter().copied().collect();

        self.multi_selected.retain(|ix| visible_set.contains(ix));
        if self.multi_selected.len() <= 1 {
            if let Some(&only) = self.multi_selected.iter().next() {
                self.selected = Some(only);
            }
            self.multi_selected.clear();
        }

        let selected_visible = self.selected.is_some_and(|ix| visible_set.contains(&ix));
        if !selected_visible {
            self.selected = visible.first().copied();
        }
        // Type-to-filter is a keyboard interaction, same as ↑/↓ — the
        // newly (re)selected row must stay on screen as the filter
        // narrows or widens the list out from under it. See
        // `scroll_selected_into_view`'s doc for why mouse-driven selection
        // never calls this.
        if let Some(display_ix) = self
            .selected
            .and_then(|sel| visible.iter().position(|&r| r == sel))
        {
            self.scroll_selected_into_view(display_ix);
        }
        self.load_details_for_selection(cx);
        cx.notify();
    }

    pub(super) fn on_focus_filter(
        &mut self,
        _: &FocusFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_open() {
            return;
        }
        let focus = self.filter_input.focus_handle(cx);
        window.focus(&focus);
        cx.notify();
    }

    /// Clear the filter field and hand focus back to the list — the
    /// ✕ button's click handler, and the filter field's own `Cancel`
    /// (Escape) reaction wired in `new`.
    pub(super) fn clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.clamp_selection_to_filter(cx);
        window.focus(&self.focus_handle);
        cx.notify();
    }

    // -------------------------------------------------------------
    // Multi-select
    // -------------------------------------------------------------

    /// The effective selection: `multi_selected` once shift/cmd-click has
    /// built one (always >1 entry by construction — see
    /// `apply_selection_set`), otherwise just `selected` alone.
    pub(super) fn selected_indices(&self) -> Vec<usize> {
        if self.multi_selected.is_empty() {
            self.selected.into_iter().collect()
        } else {
            self.multi_selected.iter().copied().collect()
        }
    }

    pub(super) fn is_row_selected(&self, row_ix: usize) -> bool {
        if self.multi_selected.is_empty() {
            self.selected == Some(row_ix)
        } else {
            self.multi_selected.contains(&row_ix)
        }
    }

    /// ⌘-click: toggle one row's membership in the selection, moving the
    /// anchor to it so a following shift-click extends from here — the
    /// same "last-touched row becomes the anchor" behavior Finder uses.
    /// This is also what the row checkbox's click handler calls and what
    /// a worktree row's "Select"/"Add to Selection"/"Remove from
    /// Selection" context-menu item dispatches to — both discoverable,
    /// mouse-only stand-ins for this same modifier-click.
    pub(super) fn toggle_row_selection(&mut self, row_ix: usize, cx: &mut Context<Self>) {
        if row_ix >= self.rows.len() {
            return;
        }
        let current = self.selected_indices();
        let set = Self::toggled_selection(&current, row_ix);
        self.apply_selection_set(set, row_ix, cx);
    }

    /// The pure toggle at the heart of `toggle_row_selection` above: flip
    /// `row_ix`'s membership in `current` without disturbing any other row.
    /// Split out from `Context`-dependent state (`self.rows`, `cx.notify`)
    /// so the actual set arithmetic — the "toggle" a checkbox click, a
    /// ⌘-click, and a context-menu "Add to Selection" all reduce to — is
    /// directly testable rather than only reachable through a live
    /// `WtmApp`.
    fn toggled_selection(current: &[usize], row_ix: usize) -> BTreeSet<usize> {
        let mut set: BTreeSet<usize> = current.iter().copied().collect();
        if !set.remove(&row_ix) {
            set.insert(row_ix);
        }
        set
    }

    /// Shift-click: select every *visible* row between the current anchor
    /// and `row_ix`, inclusive. Ranges over `visible_row_indices` rather
    /// than a raw index span so a range spanning a filtered-out worktree
    /// can never silently select a row the user cannot even see.
    pub(super) fn extend_selection_range(&mut self, row_ix: usize, cx: &mut Context<Self>) {
        let anchor = self.selected.unwrap_or(row_ix);
        let visible = self.visible_row_indices(cx);
        let (Some(a_pos), Some(b_pos)) = (
            visible.iter().position(|&r| r == anchor),
            visible.iter().position(|&r| r == row_ix),
        ) else {
            self.select(row_ix, cx);
            return;
        };
        let (lo, hi) = (a_pos.min(b_pos), a_pos.max(b_pos));
        let set: BTreeSet<usize> = visible[lo..=hi].iter().copied().collect();
        // The anchor stays put across repeated shift-clicks — passing it
        // (not `row_ix`) as the new anchor is a no-op when it is already
        // `self.selected`, which is the point: only a plain click or a
        // ⌘-click ever moves it.
        self.apply_selection_set(set, anchor, cx);
    }

    /// Apply a computed selection set, collapsing back to the plain
    /// single-selection representation when it has shrunk to ≤1 row —
    /// `multi_selected` is either empty or holds more than one index,
    /// never exactly one, so every other method here can tell "is this a
    /// multi-selection" from `multi_selected.is_empty()` alone.
    fn apply_selection_set(&mut self, set: BTreeSet<usize>, anchor: usize, cx: &mut Context<Self>) {
        if set.len() <= 1 {
            self.selected = set.into_iter().next().or(Some(anchor));
            self.multi_selected.clear();
        } else {
            self.selected = Some(anchor);
            self.multi_selected = set;
        }
        self.load_details_for_selection(cx);
        cx.notify();
    }

    /// Plain, single-row selection: replaces whatever selection existed
    /// before, collapsing any multi-selection in progress — the common
    /// case, and what every existing single-target action (detail panel,
    /// ⌘⌫, Open in Editor) already assumes `selected` alone describes.
    pub(super) fn select(&mut self, row_ix: usize, cx: &mut Context<Self>) {
        if row_ix < self.rows.len() {
            self.selected = Some(row_ix);
            self.multi_selected.clear();
            self.load_details_for_selection(cx);
            cx.notify();
        }
    }

    /// Moves among *visible* rows only — see `visible_row_indices` — so
    /// filtering the list can never let ↑/↓ land on a hidden row. Also
    /// reachable while the filter field itself has focus: `TextInput`
    /// binds no up/down of its own, so the keystroke bubbles up to this
    /// binding on the root, giving "type to filter, arrow keys to move
    /// through the results" for free.
    pub(super) fn on_select_next(
        &mut self,
        _: &SelectNext,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_open() || self.rows.is_empty() {
            return;
        }
        let visible = self.visible_row_indices(cx);
        let Some(&first) = visible.first() else {
            return;
        };
        let (next, display_ix) = match self
            .selected
            .and_then(|ix| visible.iter().position(|&r| r == ix))
        {
            Some(pos) if pos + 1 < visible.len() => (visible[pos + 1], pos + 1),
            Some(pos) => (visible[pos], pos),
            None => (first, 0),
        };
        self.select(next, cx);
        self.scroll_selected_into_view(display_ix);
    }

    pub(super) fn on_select_prev(
        &mut self,
        _: &SelectPrev,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_open() || self.rows.is_empty() {
            return;
        }
        let visible = self.visible_row_indices(cx);
        let Some(&first) = visible.first() else {
            return;
        };
        let (prev, display_ix) = match self
            .selected
            .and_then(|ix| visible.iter().position(|&r| r == ix))
        {
            Some(pos) if pos > 0 => (visible[pos - 1], pos - 1),
            Some(pos) => (visible[pos], pos),
            None => (first, 0),
        };
        self.select(prev, cx);
        self.scroll_selected_into_view(display_ix);
    }

    /// Whether the row at `display_ix` — its 0-based position in the
    /// *visible* (filtered) list the `uniform_list` is currently painting,
    /// not an index into `self.rows` — is already fully inside the list's
    /// last-painted viewport. Pure so "don't scroll a row that's already
    /// on screen" has one tested home instead of being re-derived at each
    /// keyboard-selection call site; `viewport_height` of `0.0` (nothing
    /// painted yet) always reports "not visible", so the very first
    /// keyboard move after launch still establishes a sane scroll position
    /// rather than silently no-op-ing.
    pub(super) fn row_needs_scroll(
        display_ix: usize,
        scroll_offset_y: f32,
        viewport_height: f32,
    ) -> bool {
        if viewport_height <= 0.0 {
            return true;
        }
        let item_top = theme::LIST_ROW_PITCH * display_ix as f32;
        let item_bottom = item_top + theme::LIST_ROW_HEIGHT;
        let scroll_top = -scroll_offset_y;
        item_top < scroll_top || item_bottom > scroll_top + viewport_height
    }

    /// Bring the worktree list's row at `display_ix` into view. Called only
    /// from *keyboard*-driven selection moves (`on_select_next`/
    /// `on_select_prev`, the type-to-filter field's
    /// `clamp_selection_to_filter`) — never from a mouse click's own
    /// `select` call, because a clicked row is already on screen and
    /// scrolling on click would be motion with no purpose.
    ///
    /// `row_needs_scroll` decides up front whether anything needs to move:
    /// `UniformListScrollHandle::scroll_to_item` (non-strict) already
    /// no-ops internally when the row is visible, but checking first means
    /// an already-visible selection move schedules no deferred scroll (and
    /// so no extra relayout) at all, in the spirit of the motion catalog's
    /// "no repaint loops" restraint rule. `ScrollStrategy::Top` only ever
    /// matters as a fallback for a jump larger than one row (e.g. a filter
    /// keystroke that moves the selection many rows at once); for a
    /// single-step ↑/↓ the non-strict edge-alignment gpui does internally
    /// already produces the minimal scroll regardless of which strategy is
    /// named here.
    pub(super) fn scroll_selected_into_view(&mut self, display_ix: usize) {
        let (offset_y, viewport_height) = {
            let state = self.list_scroll.0.borrow();
            (
                f32::from(state.base_handle.offset().y),
                state
                    .last_item_size
                    .map(|size| f32::from(size.item.height))
                    .unwrap_or(0.0),
            )
        };
        if Self::row_needs_scroll(display_ix, offset_y, viewport_height) {
            self.list_scroll
                .scroll_to_item(display_ix, ScrollStrategy::Top);
        }
    }

    /// Move keyboard focus to the next Tab stop (`FocusNext`'s doc explains
    /// why this binding has to be added by hand — gpui-0.2.2 has the
    /// machinery but no default keymap entry for it). No `overlay_open()`
    /// guard needed the way `on_select_next` above has one:
    /// `Theme::tab_stops` (see its doc) already keeps the background
    /// shell's own controls out of the tab order while a dialog covers it,
    /// so `Window::focus_next` naturally stays within whichever overlay is
    /// open without this handler needing to know that.
    pub(super) fn on_focus_next(
        &mut self,
        _: &FocusNext,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        window.focus_next();
    }

    /// Shift-Tab counterpart to [`Self::on_focus_next`].
    pub(super) fn on_focus_prev(
        &mut self,
        _: &FocusPrev,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        window.focus_prev();
    }

    // -------------------------------------------------------------
    // Sorting
    // -------------------------------------------------------------

    /// Change the active sort mode and re-sort `rows` immediately — not
    /// waiting for the next reload — so the toolbar's sort control feels
    /// instantaneous. A no-op when `mode` already matches: re-sorting an
    /// already-correctly-ordered list would still walk every row and (via
    /// `resort_preserving_selection`) force a repaint for nothing.
    pub(super) fn set_sort_mode(&mut self, mode: SortMode, cx: &mut Context<Self>) {
        if self.sort_mode == mode {
            return;
        }
        self.sort_mode = mode;
        self.resort_preserving_selection(cx);
    }

    /// Re-sort `self.rows` per the current `sort_mode`, translating the
    /// selection across the reorder by worktree *path* rather than index.
    ///
    /// Unlike a fresh listing (`apply_rows`, which reorders a row set that
    /// may genuinely have changed and so falls back to a clamped index), a
    /// sort-mode change reorders the *exact same* rows the user was already
    /// looking at — "the worktree I had selected is still selected" is a
    /// real, checkable promise here. So this looks the selection back up by
    /// `path` (a worktree's actual identity) after sorting, dropping an
    /// entry whose path isn't found rather than guessing at one.
    pub(super) fn resort_preserving_selection(&mut self, cx: &mut Context<Self>) {
        let (anchor_path, multi_paths) = self.selection_paths();
        worktree_list::sort_rows(&mut self.rows, self.sort_mode, &self.activity);
        self.restore_selection_by_path(anchor_path.as_deref(), &multi_paths);
        cx.notify();
    }

    /// Capture the current selection as worktree *paths* rather than
    /// indices — the identity that survives a reorder or a wholesale
    /// row-set replacement (see `restore_selection_by_path`). Shared by
    /// `resort_preserving_selection` above and `loading::apply_rows`.
    pub(super) fn selection_paths(&self) -> (Option<PathBuf>, Vec<PathBuf>) {
        let anchor = self
            .selected
            .and_then(|ix| self.rows.get(ix))
            .map(|row| row.path.clone());
        let multi = self
            .multi_selected
            .iter()
            .filter_map(|&ix| self.rows.get(ix).map(|row| row.path.clone()))
            .collect();
        (anchor, multi)
    }

    /// Look `anchor`/`multi` back up in the current (already reordered or
    /// replaced) `rows` by path, restoring `selected`/`multi_selected` to
    /// whichever indices those paths now occupy — clearing either one
    /// whose path isn't found rather than guessing at a replacement.
    pub(super) fn restore_selection_by_path(&mut self, anchor: Option<&Path>, multi: &[PathBuf]) {
        self.selected = anchor.and_then(|path| self.rows.iter().position(|r| r.path == path));
        self.multi_selected = multi
            .iter()
            .filter_map(|path| self.rows.iter().position(|r| &r.path == path))
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------
    // `row_needs_scroll` — arrow-key selection scrolling
    // -------------------------------------------------------------

    /// A viewport that exactly fits rows 0, 1, and 2 at the real on-screen
    /// pitch (row height plus its bottom padding) — row 3 starts here.
    const THREE_ROW_VIEWPORT: f32 = theme::LIST_ROW_PITCH * 2.0 + theme::LIST_ROW_HEIGHT;
    /// Scroll offset that puts that same three-row window at rows 3, 4, 5
    /// instead of 0, 1, 2.
    const SCROLLED_THREE_ROWS: f32 = -(theme::LIST_ROW_PITCH * 3.0);

    #[test]
    fn row_needs_scroll_is_false_for_a_row_already_fully_in_view() {
        // Scrolled to the very top: rows 0, 1, 2 are all fully visible.
        assert!(!WtmApp::row_needs_scroll(0, 0.0, THREE_ROW_VIEWPORT));
        assert!(!WtmApp::row_needs_scroll(2, 0.0, THREE_ROW_VIEWPORT));
    }

    #[test]
    fn row_needs_scroll_is_true_when_the_row_is_below_the_viewport() {
        // Row 3 starts exactly at the bottom edge of the three-row
        // viewport scrolled to the top — its bottom edge is off screen, so
        // it needs a scroll.
        assert!(WtmApp::row_needs_scroll(3, 0.0, THREE_ROW_VIEWPORT));
    }

    #[test]
    fn row_needs_scroll_is_true_when_the_row_is_above_the_viewport() {
        // Scrolled down by three rows: row 2 sits entirely above the
        // now-visible window.
        assert!(WtmApp::row_needs_scroll(
            2,
            SCROLLED_THREE_ROWS,
            THREE_ROW_VIEWPORT
        ));
    }

    #[test]
    fn row_needs_scroll_is_false_once_scrolled_to_show_it() {
        // Same scroll position as above, but row 3 is now exactly the top
        // row of the viewport.
        assert!(!WtmApp::row_needs_scroll(
            3,
            SCROLLED_THREE_ROWS,
            THREE_ROW_VIEWPORT
        ));
    }

    #[test]
    fn row_needs_scroll_is_true_before_any_layout_has_painted() {
        // `viewport_height <= 0.0` models the first frame, before
        // `UniformListScrollHandle` has recorded any real geometry — never
        // trust "already visible" against numbers that were never real.
        assert!(WtmApp::row_needs_scroll(0, 0.0, 0.0));
    }

    #[test]
    fn row_needs_scroll_steps_by_the_row_pitch_not_the_row_height() {
        // A viewport that fits exactly 3 rows at `LIST_ROW_HEIGHT` alone
        // (168.0) is one row short of fitting 3 rows at the real on-screen
        // pitch (row height plus its bottom padding). Row 2 therefore
        // needs a scroll here; stepping by `LIST_ROW_HEIGHT` instead of
        // `LIST_ROW_PITCH` would place its bottom edge exactly on the
        // viewport boundary and wrongly call it already visible.
        let viewport_height = theme::LIST_ROW_HEIGHT * 3.0;
        assert!(WtmApp::row_needs_scroll(2, 0.0, viewport_height));
    }

    #[test]
    fn toggled_selection_adds_an_unselected_row_without_disturbing_others() {
        let set = WtmApp::toggled_selection(&[1, 2], 3);
        assert_eq!(set, BTreeSet::from([1, 2, 3]));
        let set = WtmApp::toggled_selection(&[], 5);
        assert_eq!(set, BTreeSet::from([5]));
    }

    #[test]
    fn toggled_selection_removes_an_already_selected_row_without_disturbing_others() {
        let set = WtmApp::toggled_selection(&[1, 2, 3], 2);
        assert_eq!(set, BTreeSet::from([1, 3]));
        let set = WtmApp::toggled_selection(&[0, 4, 7], 4);
        assert_eq!(set, BTreeSet::from([0, 7]));
    }
}

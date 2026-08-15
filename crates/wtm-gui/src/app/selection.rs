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
        let query = self.filter_input.read(cx).value().trim().to_string();
        if query.is_empty() {
            return (0..self.rows.len()).collect();
        }
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| palette::fuzzy_match(&query, row.display_name()).is_some())
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
    pub(super) fn toggle_row_selection(&mut self, row_ix: usize, cx: &mut Context<Self>) {
        if row_ix >= self.rows.len() {
            return;
        }
        let mut set: BTreeSet<usize> = self.selected_indices().into_iter().collect();
        if !set.remove(&row_ix) {
            set.insert(row_ix);
        }
        self.apply_selection_set(set, row_ix, cx);
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
        let next = match self
            .selected
            .and_then(|ix| visible.iter().position(|&r| r == ix))
        {
            Some(pos) if pos + 1 < visible.len() => visible[pos + 1],
            Some(pos) => visible[pos],
            None => first,
        };
        self.select(next, cx);
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
        let prev = match self
            .selected
            .and_then(|ix| visible.iter().position(|&r| r == ix))
        {
            Some(pos) if pos > 0 => visible[pos - 1],
            Some(pos) => visible[pos],
            None => first,
        };
        self.select(prev, cx);
    }
}

//! The window's chrome: the sidebar (repository list), the title bar, the
//! worktree list itself, and the footer status line. Also the detail
//! panel's `show`/`render` pair, since both are one-line, view-only
//! predicates over `self.details`.
//!
//! This is pure-ish rendering — it reads `WtmApp` state and produces
//! elements, wiring clicks back through `cx.listener` to methods defined
//! in `selection` and `commands`. Dialog-specific rendering (the modal
//! forms) is deliberately not here; see `dialog_forms`.

use super::*;

use crate::motion;

/// The worktree list's own content-column cap (`render_list`'s "a bounded
/// content column: on a wide window, full-width rows strand the status
/// pills a screen away from the branch they describe"). Named so
/// [`WtmApp::worktree_row_card_width`] can reuse the exact number
/// `render_list`'s `max_w(px(..))` paints, instead of a second literal that
/// could silently drift from it.
const LIST_MAX_WIDTH: f32 = 1040.0;

/// [`WtmApp::render_row_checkbox`]'s own fixed square size, named so
/// [`WtmApp::worktree_row_card_width`] can reserve the exact same width
/// instead of a second, independently-typed `15.0`.
const ROW_CHECKBOX_SIZE: f32 = 15.0;

impl WtmApp {
    /// The sidebar: window controls clearance, actions, then the repo list.
    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.chrome_theme(cx);
        let active_path = self.active.as_ref().map(|r| r.path().to_path_buf());

        // Continuity (SPEC §5 candidate 1): the sidebar mounts/unmounts
        // instantly today, with no explanation of where it went. Wrapped in
        // `motion::pane_in` below — see that helper's doc for why this
        // animates opacity + a slide rather than the `w(px(..))` set here,
        // which stays instant so every layout budget that reads
        // `theme::SIDEBAR_WIDTH` (`app::layout::content_column_width`,
        // `worktree_row_card_width`) is correct from frame one.
        let sidebar = div()
            .w(px(theme::SIDEBAR_WIDTH))
            .h_full()
            .flex()
            .flex_none()
            .flex_col()
            // The chrome plane, translucent over the window's blurred
            // backing on macOS (opaque elsewhere) — SURFACES §1.
            .bg(theme.glass())
            .border_r_1()
            .border_color(theme.border)
            // The title bar is transparent, so the sidebar starts under the
            // traffic lights and has to leave room for them.
            .child(div().h(px(ui::TITLEBAR_HEIGHT)).flex_none())
            .child(
                div()
                    .px(px(theme::SPACE_8))
                    .flex()
                    .flex_col()
                    .child(
                        ui::action_row(
                            "new-worktree",
                            icons::PLUS,
                            "New Worktree",
                            Some("⌘N"),
                            &theme,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.on_new_worktree(&NewWorktree, window, cx);
                        })),
                    )
                    .child(
                        ui::action_row("search", icons::SEARCH, "Search", Some("⌘K"), &theme)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.on_open_palette(&OpenPalette, window, cx);
                            })),
                    )
                    // The "Repositories" eyebrow (and the hand-rolled header
                    // copy it replaced before that) is gone — the user's
                    // call, final: every eyebrow in the app goes. Its `+`
                    // button is promoted here to a third `action_row`
                    // alongside "New Worktree" and "Search" rather than
                    // left to float without a header to anchor it; the
                    // sidebar already speaks this vocabulary (icon, label,
                    // shortcut chip), so this reads as consistency, not
                    // loss.
                    .child(
                        ui::action_row(
                            "add-repository",
                            icons::PLUS,
                            "Add Repository",
                            Some("⌘⇧O"),
                            &theme,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.on_add_repository(&AddRepository, window, cx);
                        })),
                    ),
            )
            .child(div().h(px(theme::SPACE_12)).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .px(px(theme::SPACE_8))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(theme::SPACE_2))
                            .children(self.repos.iter().map(|entry| {
                                self.render_repo_row(entry, active_path.as_deref(), &theme, cx)
                            }))
                            .when(self.repos.is_empty(), |this| {
                                // The empty state no longer repeats its own
                                // "Add Repository" affordance — the action
                                // row above (always visible, not scoped to
                                // "the list happens to be empty") already
                                // covers it; a second one here would just be
                                // the same control twice.
                                this.child(
                                    div()
                                        .px(px(theme::SPACE_8))
                                        .py(px(theme::SPACE_6))
                                        .text_size(px(ui::TEXT_SM))
                                        .text_color(theme.text_ghost)
                                        .child("No repositories yet."),
                                )
                            }),
                    ),
            )
            .child(
                // Height is left to derive from the button plus this
                // padding rather than a fixed density number, so it doesn't
                // need its own token — see the redesign report.
                div()
                    .flex_none()
                    .py(px(theme::SPACE_8))
                    .px(px(theme::SPACE_8))
                    .flex()
                    .items_center()
                    .child(
                        ui::icon_button_with_tooltip(
                            "settings",
                            icons::SETTINGS,
                            "Settings · ⌘,",
                            &theme,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.on_open_settings(&OpenSettings, window, cx);
                        })),
                    ),
            );

        // Enters from further off the window's left edge than its resting
        // position (`start_offset_px` negative — see `motion::pane_in`'s
        // doc), so it slides in from its own home edge.
        motion::pane_in("sidebar-pane", sidebar, -8.0, cx)
    }

    fn render_repo_row(
        &self,
        entry: &RepoEntry,
        active_path: Option<&std::path::Path>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_active = active_path == Some(entry.path.as_path());
        let missing = !entry.exists();
        let path = entry.path.clone();

        ui::row(
            SharedString::from(entry.path.display().to_string()),
            is_active,
            theme,
        )
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_4))
        .child(
            div()
                .flex()
                .min_w_0()
                .items_center()
                .gap(px(theme::SPACE_6))
                .line_height(px(18.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(ui::TEXT_BASE))
                        .text_color(if missing {
                            theme.text_faint
                        } else {
                            theme.text
                        })
                        .child(entry.name.clone()),
                )
                .when(missing, |this| {
                    // A missing worktree directory keeps its warning icon,
                    // now with a tooltip explaining what it means — an icon
                    // alone with no accessible name is exactly the defect
                    // COMPONENTS.md calls out. `.tooltip(..)` is only on
                    // `StatefulInteractiveElement` (gpui-0.2.2's
                    // `elements/div.rs`), so this needs an `.id(..)` to
                    // become `Stateful<Div>` before it's callable.
                    this.child(
                        div()
                            .id(SharedString::from(format!(
                                "repo-missing-{}",
                                entry.path.display()
                            )))
                            .child(ui::icon(icons::WARNING, 12.0, theme.warning))
                            .tooltip(ui::tooltip(
                                // Names the problem and the recovery
                                // (`better-writing`) — right-click already
                                // offers "Remove from Sidebar" for exactly
                                // this case (`open_repo_context_menu`).
                                "This repository's folder could not be found. \
                                 Right-click to remove it from the sidebar.",
                            )),
                    )
                }),
        )
        .child(div().flex().min_w_0().items_center().child(ui::meta(
            icons::FOLDER,
            parent_label(&entry.path),
            theme,
        )))
        .on_click(cx.listener({
            let path = path.clone();
            move |this, _, _window, cx| {
                this.select_repo(path.clone(), cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                this.open_repo_context_menu(path.clone(), event.position, cx);
            }),
        )
    }

    /// The title bar strip: traffic-light clearance, sidebar toggle, the
    /// active repository, and the actions that apply to it. On Linux, when
    /// the compositor has handed the app client-side decorations, this also
    /// grows a drag-to-move/double-click-to-zoom region and the window
    /// controls macOS gets from the real traffic lights instead — see
    /// `render_window_controls`.
    pub(super) fn render_titlebar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.chrome_theme(cx);
        let title = self
            .active
            .as_ref()
            .map(|repo| repo.name().to_string())
            .unwrap_or_else(|| "wtm".to_string());
        // Whether *this* window currently has client-side decorations —
        // not simply "is this Linux": an X11 window manager without
        // decoration support keeps `Decorations::Server` regardless of what
        // `main.rs` requested, and in that case the compositor's own title
        // bar already has close/minimize/maximize and its own dragging, so
        // adding ours here would be redundant. macOS never takes this
        // branch at all (see `window_frame`'s module doc), which is what
        // keeps it on real traffic lights without needing a `#[cfg]` here.
        let csd = matches!(window.window_decorations(), Decorations::Client { .. });

        div()
            .h(px(ui::TITLEBAR_HEIGHT))
            .w_full()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(theme::SPACE_8))
            .px(px(theme::SPACE_8))
            // Only the collapsed sidebar leaves the traffic lights over this
            // strip; when the sidebar is open they sit above it instead.
            // macOS only — on Linux this space belongs to content, whether
            // or not client-side decorations are in play (a title bar with
            // no OS-drawn buttons needs no clearance for any; the buttons
            // this strip draws itself under CSD are sized and placed by
            // `render_window_controls`, not by reserving space up front).
            .when(cfg!(target_os = "macos") && !self.sidebar_visible, |this| {
                this.pl(px(ui::TRAFFIC_LIGHT_CLEARANCE))
            })
            .child(
                ui::icon_button_with_tooltip(
                    "toggle-sidebar",
                    icons::PANEL_LEFT,
                    "Toggle Sidebar · ⌘B",
                    &theme,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.on_toggle_sidebar(&ToggleSidebar, window, cx);
                })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE_6))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(ui::TEXT_BASE))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child(title),
                    )
                    .when_some(self.selected_branch(), |this, branch| {
                        this.child(
                            div()
                                .flex()
                                .min_w_0()
                                .items_center()
                                .gap(px(theme::SPACE_6))
                                .text_size(px(ui::TEXT_SM))
                                .child(div().text_color(theme.text_ghost).child("/"))
                                .child(ui::meta(icons::GIT_BRANCH, branch, &theme)),
                        )
                    })
                    // Under CSD there is no native title bar left to drag by,
                    // so this — the flexible middle of the strip, which has
                    // no click handler of its own today — takes over that
                    // job: a first press starts moving the window, a second
                    // (`click_count` from the platform's own double-click
                    // detection, the same field `worktree_list`'s row
                    // double-click already reads off `ClickEvent`) zooms it
                    // instead, matching how a real title bar behaves. Scoped
                    // to this label area rather than the whole strip on
                    // purpose: every button on either side of it (including
                    // `render_window_controls`) is this element's *sibling*,
                    // not its descendant, so none of their presses ever
                    // bubble through here — nothing needs to guard against a
                    // move starting underneath a button click.
                    .when(csd, |this| {
                        this.on_mouse_down(MouseButton::Left, |event, window, _cx| {
                            if event.click_count >= 2 {
                                window.zoom_window();
                            } else {
                                window.start_window_move();
                            }
                        })
                    }),
            )
            .child(
                ui::icon_button_with_tooltip(
                    "open-selected",
                    icons::OPEN_EXTERNAL,
                    "Open in Editor · ⏎",
                    &theme,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.on_open_selected(&OpenSelected, window, cx);
                })),
            )
            .child(self.render_reload_button(&theme, cx))
            .child(
                // `panel-right` now exists in `assets.rs` — the sidebar's own
                // `panel-left` glyph was only ever a stand-in reused because
                // no dedicated icon existed yet (see git history); this uses
                // the real one.
                ui::icon_button_with_tooltip(
                    "toggle-detail-panel",
                    icons::PANEL_RIGHT,
                    "Toggle Detail Panel · ⌘I",
                    &theme,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.on_toggle_detail_panel(&ToggleDetailPanel, window, cx);
                })),
            )
            .when(csd, |this| this.child(render_window_controls(&theme, cx)))
    }

    /// The titlebar's reload button. Unlike every other titlebar icon (built
    /// through [`ui::icon_button_with_tooltip`]), this one swaps its glyph
    /// for a spinning one while `self.loading` is true and stops the instant
    /// it lands — SURFACES §2: "Reload gets a spin animation while a reload
    /// is actually running and stops when it finishes. This is the one place
    /// a spinner is honest." `self.loading` is exactly that signal: it's set
    /// at the start of every `reload_impl` (manual ⌘R, a repo switch, the
    /// filesystem watcher) and cleared once the status-bearing pass lands —
    /// see `loading.rs`.
    ///
    /// `ui::icon_button`/`icon_button_with_tooltip` hard-code a static icon
    /// child and have no animated variant, so this rebuilds their exact look
    /// (`theme::ICON_BUTTON_SIZE`, `RADIUS_CONTROL`, `element_hover` hover,
    /// `press_feedback`) by hand instead of through them — `ui.rs` is
    /// outside this task's file scope (see the redesign report) so a real
    /// animated variant belongs there, not here.
    fn render_reload_button(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        const TOOLTIP: &str = "Reload · ⌘R";

        let glyph: AnyElement = if self.loading {
            let icon_svg = gpui::svg()
                .path(icons::REFRESH)
                .size(px(14.0))
                .flex_none()
                .text_color(theme.text_muted);
            motion::spin("reload-spin", icon_svg, cx)
        } else {
            ui::icon(icons::REFRESH, 14.0, theme.text_muted).into_any_element()
        };

        let styled = div()
            .id("reload")
            .w(px(theme::ICON_BUTTON_SIZE))
            .h(px(theme::ICON_BUTTON_SIZE))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(px(theme::RADIUS_CONTROL))
            .cursor_default()
            .hover(|this| this.bg(theme.element_hover))
            .child(glyph);

        ui::press_feedback(styled, theme)
            .tooltip(ui::tooltip(TOOLTIP))
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_reload(&Reload, window, cx);
            }))
            .into_any_element()
    }

    fn selected_branch(&self) -> Option<String> {
        let info = self.rows.get(self.selected?)?;
        Some(info.display_name().to_string())
    }

    /// The live pixel width `worktree_list::render_row` actually gets for
    /// one row's card, this frame — not the flat, worst-case-only character
    /// budget the row used before (Task 1: "a fixed character cap does not
    /// adapt to width"). Recomputed once per `uniform_list` render pass
    /// (nothing in this formula varies row to row, so the call site does it
    /// once rather than once per visible row) from the window's *real*
    /// drawable width, minus whichever side panels are actually showing
    /// this frame, minus [`LIST_MAX_WIDTH`]'s centering cap, minus every
    /// fixed-size wrapper between the content column and the row card
    /// itself: the list's own `SPACE_8` inset, the per-row wrapper's own
    /// `SPACE_8` inset, and the selection checkbox
    /// ([`ROW_CHECKBOX_SIZE`]) plus its `SPACE_6` gap to the card — see
    /// `render_list`'s own `uniform_list` child for each of those in
    /// context.
    ///
    /// `worktree_list::render_row` turns this into per-row budgets (branch
    /// name / path / sha / age) using the same char-width approximation
    /// `diff_view::GUTTER_CHAR_WIDTH`/`detail_panel::FACT_VALUE_MAX_CHARS`
    /// already use — see its own doc for why an approximation is the only
    /// option (gpui has no API to measure real shaped text outside of an
    /// actual layout pass).
    fn worktree_row_card_width(&self, window: &Window) -> f32 {
        let panel_width = self
            .show_detail_panel(window)
            .then(|| self.detail_panel_width());
        let content_column = layout::content_column_width(
            f32::from(window.viewport_size().width),
            self.sidebar_visible,
            panel_width,
        )
        .min(LIST_MAX_WIDTH);
        content_column
            - theme::SPACE_8 * 2.0 // the list's own `.px(px(theme::SPACE_8))`
            - theme::SPACE_8 * 2.0 // the per-row wrapper's own `.px(px(theme::SPACE_8))`
            - ROW_CHECKBOX_SIZE
            - theme::SPACE_6 // the row wrapper's `.gap(px(theme::SPACE_6))` to the card
    }

    pub(super) fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Right-clicking the list's own background — not a row — is a
        // standard place users look for "do something here"; wiring it up
        // is worth doing even in the three empty/loading states below,
        // where "Add Repository…" (or, once a repo is open, "New
        // Worktree") is often exactly what a user reaching for a right
        // click here wants.
        let empty_space_menu = cx.listener(|this, event: &MouseDownEvent, _window, cx| {
            this.open_empty_space_context_menu(event.position, cx);
        });

        if self.active.is_none() {
            let theme = self.chrome_theme(cx);
            let action = ui::button(
                "empty-add-repository",
                "Add Repository",
                ButtonVariant::Primary,
                &theme,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_add_repository(&AddRepository, window, cx);
            }))
            .into_any_element();
            return div()
                .flex_1()
                .child(worktree_list::render_no_repo(action, cx))
                .on_mouse_down(MouseButton::Right, empty_space_menu)
                .into_any_element();
        }
        if self.rows.is_empty() && self.loading {
            // Reachable only when `seed_initial_rows` fell back to `reload`
            // at startup (a broken repo — see its doc comment) or briefly
            // during a repo switch, before the fast pass has landed. The
            // worktree count is genuinely unknown at this instant, so both
            // of the alternatives below would be claiming something false:
            // `render_header`'s "0 worktrees" (built for a confirmed empty
            // count, not an unknown one) and `render_empty`'s "No worktrees
            // yet" equally assert a fact this app doesn't have yet.
            let theme = self.chrome_theme(cx);
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Right, empty_space_menu)
                .child(
                    div()
                        .text_size(px(ui::TEXT_BASE))
                        .text_color(theme.text_muted)
                        .child("Listing worktrees…"),
                )
                .into_any_element();
        }
        if self.rows.is_empty() && !self.loading {
            let theme = self.chrome_theme(cx);
            let action = ui::button(
                "empty-new-worktree",
                "New Worktree",
                ButtonVariant::Primary,
                &theme,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_new_worktree(&NewWorktree, window, cx);
            }))
            .into_any_element();
            return div()
                .flex_1()
                .child(worktree_list::render_empty(action, cx))
                .on_mouse_down(MouseButton::Right, empty_space_menu)
                .into_any_element();
        }

        let theme = self.chrome_theme(cx);
        // The filter field stays mounted (and painted) behind an open
        // dialog/palette/context menu, exactly like every `ui.rs` row and
        // button on this surface — see `TextInput::set_tab_stop`'s doc for
        // why it needs its own per-render toggle rather than picking this
        // up from `theme.tab_stops` automatically the way those do.
        self.filter_input
            .update(cx, |input, cx| input.set_tab_stop(theme.tab_stops, cx));
        let visible = self.visible_row_indices(cx);
        let shown = visible.len();
        let total = self.rows.len();
        let filter_active = !self.filter_input.read(cx).value().trim().is_empty();
        // The same baseline `dialogs::PruneState::new()` starts with
        // (`merged`/`gone` both off), so this count never promises more
        // than what clicking through to the Prune dialog will actually
        // show by default.
        let prunable = self
            .active
            .as_ref()
            .map(|repo| prunable_count(repo, &self.rows))
            .unwrap_or(0);
        let prune_label = if prunable > 0 {
            format!("Prune… ({prunable})")
        } else {
            "Prune…".to_string()
        };
        let multi_count = self.multi_selected.len();

        // A bounded content column: on a wide window, full-width rows strand
        // the status pills a screen away from the branch they describe.
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(LIST_MAX_WIDTH))
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        // FINDINGS.md G1: at 900×600 (the window's own
                        // minimum) this row used to lose the sort control
                        // and the filter field off the right edge entirely —
                        // `flex_wrap()` was already here but never fired.
                        // Root cause (see FINDINGS-2.md's "mechanism"
                        // section): a flex child never shrinks below its own
                        // content width unless `min_w_0()` is set on *every*
                        // element in the chain down to the text node: this
                        // row, its `max_w(1040)` and `flex_1` ancestors
                        // above, the count (now truncating for real —
                        // `worktree_list::render_header` grew its own
                        // `min_w_0`/`.truncate()` this phase instead of
                        // relying on this wrapper's `overflow_hidden` to
                        // hard-clip it), and the actions group below. Without
                        // that chain, nothing ever reported as "out of room",
                        // so the wrap this row already asked for never had a
                        // reason to trigger.
                        //
                        // Chosen degrade strategy: **wrap**, not
                        // priority-collapse. Every control here (New
                        // Worktree, Fetch, Prune, the sort segmented control,
                        // the filter field) is something a user reaches for
                        // regularly — there's no single "lowest-value"
                        // control to demote into an overflow menu without
                        // that menu becoming just as likely to be reached
                        // for, and building one would mean new interactive
                        // surface in `ui.rs`, which this phase does not own.
                        // Wrapping keeps every control reachable with a
                        // fixed, predictable set of elements: the count
                        // gives way first (it already shrinks/truncates),
                        // then the actions group drops to its own line, then
                        // — since even a full line is not always wide enough
                        // for five controls at once — the actions group's
                        // own `flex_wrap()` (below) lets individual controls
                        // spill onto a third line rather than clip. The
                        // `SPACE_16` gap between the two top-level children
                        // is 2× the `SPACE_8` gap used *within* the actions
                        // group, per `better-layout` §1.
                        div()
                            .flex()
                            .flex_wrap()
                            .min_w_0()
                            .items_center()
                            .justify_between()
                            .gap(px(theme::SPACE_16))
                            .px(px(theme::SPACE_16))
                            .pb(px(theme::SPACE_8))
                            .child(div().flex_1().min_w_0().child(worktree_list::render_header(
                                shown,
                                total,
                                self.loading,
                                cx,
                            )))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .min_w_0()
                                    .items_center()
                                    .gap(px(theme::SPACE_8))
                                    .child(
                                        // The view's one filled control —
                                        // SURFACES §3: "New Worktree is the
                                        // only filled button in the view."
                                        ui::toolbar_button(
                                            "toolbar-new-worktree",
                                            icons::PLUS,
                                            "New Worktree",
                                            ButtonVariant::Primary,
                                            &theme,
                                        )
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.on_new_worktree(&NewWorktree, window, cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        // Label and appearance both change while a
                                        // fetch is running, but the real guard
                                        // against a second concurrent `git fetch` is
                                        // `on_fetch_remote`'s own `self.fetching`
                                        // check — this is only the visible half of
                                        // that promise.
                                        ui::toolbar_button(
                                            "toolbar-fetch",
                                            icons::REFRESH,
                                            if self.fetching {
                                                "Fetching…"
                                            } else {
                                                "Fetch"
                                            },
                                            ButtonVariant::Secondary,
                                            &theme,
                                        )
                                        .when(self.fetching, |this| this.opacity(0.6))
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.on_fetch_remote(&FetchRemote, window, cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        // Opens the same confirm-with-toggles dialog
                                        // as the shortcut/menu path always has —
                                        // pruning without a confirmation step would
                                        // be destructive, so this button is a
                                        // discoverable door to that dialog, not a
                                        // way around it. The count in the label is
                                        // what lets a user see there is something to
                                        // clean without opening anything. Secondary,
                                        // never filled — SURFACES §3.
                                        ui::toolbar_button(
                                            "toolbar-prune",
                                            icons::TRASH,
                                            prune_label,
                                            ButtonVariant::Secondary,
                                            &theme,
                                        )
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.on_prune_repo(&PruneRepo, window, cx);
                                            }),
                                        ),
                                    )
                                    .child(self.render_sort_control(&theme, cx))
                                    .child(
                                        // `200.0` is a deliberate field width,
                                        // not a spacing/radius/text value, so
                                        // it has no `SPACE_*` token to draw
                                        // from — left as-is; see the redesign
                                        // report.
                                        div()
                                            .w(px(200.0))
                                            .flex_none()
                                            .child(self.filter_input.clone()),
                                    )
                                    .when(filter_active, |this| {
                                        this.child(
                                            ui::icon_button_with_tooltip(
                                                "clear-filter",
                                                icons::CLOSE,
                                                "Clear Filter",
                                                &theme,
                                            )
                                            .on_click(
                                                cx.listener(|this, _, window, cx| {
                                                    this.clear_filter(window, cx)
                                                }),
                                            ),
                                        )
                                    }),
                            ),
                    )
                    .when(multi_count > 1, |this| {
                        this.child(self.render_selection_bar(multi_count, &theme, cx))
                    })
                    .child({
                        // `.relative()` wrapper, sibling (not ancestor) of
                        // the scrolling `uniform_list` itself — the
                        // scrollbar/fade overlays below must never be
                        // descendants of the div that actually scrolls, or
                        // they would scroll away with the very content
                        // they're annotating (`ui::scrollbar`'s own doc).
                        let list_handle = self.list_scroll.0.borrow().base_handle.clone();
                        let edges = ui::scroll_edges(
                            f32::from(list_handle.offset().y),
                            f32::from(list_handle.max_offset().height),
                        );
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .child(
                                uniform_list(
                                    "worktrees",
                                    shown,
                                    cx.processor(
                                        |this, range: std::ops::Range<usize>, window, cx| {
                                            let visible = this.visible_row_indices(cx);
                                            let theme = this.chrome_theme(cx);
                                            // Computed once per visible range rather
                                            // than once per row: every row's age is
                                            // relative to the same "now", and a fresh
                                            // syscall per row would be pure waste.
                                            let now = std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .map(|d| d.as_secs() as i64)
                                                .unwrap_or(0);
                                            // "Whenever any multi-selection is active" —
                                            // `multi_selected` is never exactly one
                                            // element (see `apply_selection_set`), so
                                            // this is the same "is this a real,
                                            // 2-or-more multi-selection" test the footer
                                            // chip and the selection bar above use.
                                            let force_checkbox_visible =
                                                !this.multi_selected.is_empty();
                                            // Nothing in this formula varies row to row
                                            // (Task 1) — computed once per visible range,
                                            // like `now` above, not once per row.
                                            let card_width = this.worktree_row_card_width(window);
                                            range
                                                .map(|display_ix| {
                                                    let ix = visible[display_ix];
                                                    let selected = this.is_row_selected(ix);
                                                    let age = this
                                                        .activity
                                                        .get(&this.rows[ix].path)
                                                        .map(|&t| data::relative_age(t, now));
                                                    // Unique per row rather than one
                                                    // shared name: `uniform_list`
                                                    // recycles element identities across
                                                    // scroll positions, and a shared
                                                    // group name would make every row's
                                                    // checkbox reveal together the
                                                    // moment any one of them is hovered.
                                                    let group_name =
                                                        SharedString::from(format!("wt-row-{ix}"));
                                                    div()
                                            .px(px(theme::SPACE_8))
                                            .pb(px(2.0))
                                            .flex()
                                            .items_center()
                                            .gap(px(theme::SPACE_6))
                                            .group(group_name.clone())
                                            .child(Self::render_row_checkbox(
                                                ix,
                                                selected,
                                                force_checkbox_visible,
                                                group_name,
                                                &theme,
                                                cx,
                                            ))
                                            .child(
                                                worktree_list::render_row(
                                                    &this.rows[ix],
                                                    ix,
                                                    selected,
                                                    this.awaiting_status,
                                                    age,
                                                    card_width,
                                                    &theme,
                                                    cx,
                                                )
                                                .flex_1()
                                                .min_w_0()
                                                .on_click(cx.listener(
                                                    move |this, event: &ClickEvent, _, cx| {
                                                        let modifiers = event.modifiers();
                                                        if modifiers.shift {
                                                            this.extend_selection_range(ix, cx);
                                                        } else if modifiers.platform {
                                                            this.toggle_row_selection(ix, cx);
                                                        } else {
                                                            this.select(ix, cx);
                                                            // Double click activates,
                                                            // matching Enter: open it
                                                            // in the editor. Only for
                                                            // a plain click — shift/⌘
                                                            // build a selection, they
                                                            // never open anything.
                                                            if event.click_count() >= 2 {
                                                                this.open_row_in_editor(ix, cx);
                                                            }
                                                        }
                                                    },
                                                )),
                                            )
                                            // On the row's full width (the
                                            // checkbox gutter included), not
                                            // just the card, so right-clicking
                                            // anywhere on the row opens this
                                            // menu rather than occasionally
                                            // falling through to the list
                                            // background's own right-click
                                            // handler below.
                                            .on_mouse_down(
                                                MouseButton::Right,
                                                cx.listener(
                                                    move |this, event: &MouseDownEvent, _, cx| {
                                                        this.open_worktree_context_menu(
                                                            ix,
                                                            event.position,
                                                            cx,
                                                        );
                                                        cx.stop_propagation();
                                                    },
                                                ),
                                            )
                                                })
                                                .collect()
                                        },
                                    ),
                                )
                                .track_scroll(self.list_scroll.clone())
                                .flex_1()
                                .px(px(theme::SPACE_8))
                                .on_mouse_down(MouseButton::Right, empty_space_menu),
                            )
                            .when(edges.leading, |this| {
                                this.child(ui::scroll_fade_top(theme.bg, theme::SPACE_24))
                            })
                            .when(edges.trailing, |this| {
                                this.child(ui::scroll_fade_bottom(theme.bg, theme::SPACE_24))
                            })
                            .child(ui::scrollbar(
                                "worktree-list-scrollbar",
                                &list_handle,
                                ui::ScrollAxis::Vertical,
                            ))
                    }),
            )
            .into_any_element()
    }

    /// The list toolbar's sort-mode switch: a compact three-way segmented
    /// control (Name / Recent / Status). Clicking a segment goes through
    /// `selection::set_sort_mode`, which re-sorts immediately and keeps the
    /// current selection on the same worktree — this control itself has
    /// nothing to do about that, or about the main worktree staying pinned
    /// first (both are `worktree_list::sort_rows`'s own guarantees).
    ///
    /// Now literally `ui::segmented`: its generic `on_select` closure is
    /// exactly the shape `cx.listener` produces (`impl Fn(&E, &mut Window,
    /// &mut App)`), so wiring the real `set_sort_mode` click through it
    /// needs no signature change to `ui.rs` — the hand-rolled copy this
    /// used to be is gone.
    fn render_sort_control(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let options: Vec<(SortMode, &str)> = SortMode::ALL
            .into_iter()
            .map(|mode| (mode, worktree_list::sort_mode_label(mode)))
            .collect();
        ui::segmented(
            "sort-mode",
            &options,
            &self.sort_mode,
            theme,
            cx.listener(|this, mode: &SortMode, _window, cx| {
                this.set_sort_mode(*mode, cx);
            }),
        )
        .into_any_element()
    }

    /// The mouse-discoverable equivalent of a ⌘-click: a small checkbox at
    /// the left edge of each row that toggles that row's membership in the
    /// multi-selection (`selection::toggle_row_selection`) without
    /// disturbing the rest of it. Hidden at rest — `group_name` ties its
    /// visibility to hovering anywhere on the row it belongs to, via
    /// `group_hover` — except when `force_visible` (a real multi-selection
    /// is already active), in which case every row's checkbox stays on
    /// screen so the whole selection reads at a glance without having to
    /// hover each row in turn.
    fn render_row_checkbox(
        row_ix: usize,
        checked: bool,
        force_visible: bool,
        group_name: SharedString,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(("row-checkbox", row_ix))
            .flex_none()
            .w(px(ROW_CHECKBOX_SIZE))
            .h(px(ROW_CHECKBOX_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .cursor_default()
            .border_1()
            // FINDINGS F4: the accent is identity/focus only (SPEC §3) and
            // must never be a structural fill — a checked checkbox is a
            // selection state, already carried by the row's own wash. So
            // the plate stays neutral (`element_active`, the same wash
            // every other selected state in the app uses) and only the
            // border and the check glyph itself carry the accent, rather
            // than filling the whole square with it.
            .border_color(if checked {
                theme.accent
            } else {
                theme.border_strong
            })
            .bg(if checked {
                theme.element_active
            } else {
                gpui::transparent_black()
            })
            .when(!checked && !force_visible, |this| this.opacity(0.0))
            .group_hover(group_name, |style| style.opacity(1.0))
            .when(checked, |this| {
                this.child(ui::icon(icons::CHECK, 10.0, theme.accent))
            })
            .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                this.toggle_row_selection(row_ix, cx);
                // The checkbox's whole point is toggling *this* row without
                // otherwise touching the click — it must not also reach the
                // card's own `on_click` (a plain-click select) were one ever
                // added as an ancestor of this element.
                cx.stop_propagation();
            }))
    }

    /// Shown between the toolbar and the list once a real multi-selection
    /// (2+ rows, by `apply_selection_set`'s own invariant) exists: how many
    /// rows are selected, and the two things you can do with the whole
    /// batch at once — the discoverable surface for what a shift/⌘-click,
    /// or the new row checkboxes, just built. The bulk-remove path itself
    /// already exists (`RemoveSelected` already branches on a multi-row
    /// selection); this only wires a visible button to it.
    fn render_selection_bar(
        &self,
        count: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(theme::SPACE_8))
            .px(px(theme::SPACE_16))
            .pb(px(theme::SPACE_8))
            .child(ui::meta(icons::CHECK, format!("{count} selected"), theme))
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(px(theme::SPACE_8))
                    .child(
                        // Lowest-priority text in this row — the reminder
                        // shrinks and clips before either button does
                        // (`ui::button` is already `flex_none`), the same
                        // "give up space gracefully" discipline FINDINGS F1
                        // asked for in the toolbar above.
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(ui::TEXT_XS))
                            .text_color(theme.text_ghost)
                            .child("⇧-click extends · ⌘-click toggles · ⎋ clears"),
                    )
                    .child(
                        ui::button("selection-clear", "Clear", ButtonVariant::Secondary, theme)
                            // `close_dialog`'s own fallback — nothing else is
                            // open, so this reaches straight through to
                            // "collapse the multi-selection", the same thing
                            // Escape already does; reusing it here keeps
                            // that behavior defined in exactly one place.
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_dialog(window, cx)),
                            ),
                    )
                    .child(
                        ui::button(
                            "selection-remove",
                            "Remove Selected",
                            ButtonVariant::Danger,
                            theme,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.on_remove_selected(&RemoveSelected, window, cx);
                        })),
                    ),
            )
    }

    /// The footer: the current message on the left, context chips on the
    /// right, in the spirit of a status line that never shouts (SURFACES
    /// §5: "ambient information — it must never out-shout the list").
    pub(super) fn render_footer(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.chrome_theme(cx);
        // The footer spans the same content column the list/titlebar above
        // it do (`app/mod.rs`'s root layout puts all three in one
        // `flex_1` child) — reusing `layout::content_column_width` rather
        // than a second copy of the sidebar/panel subtraction lets the
        // hint row degrade by the same width signal the detail-panel
        // collapse uses, instead of drifting out of sync with it.
        let panel_width = self
            .show_detail_panel(window)
            .then(|| self.detail_panel_width());
        let content_column = layout::content_column_width(
            f32::from(window.viewport_size().width),
            self.sidebar_visible,
            panel_width,
        );

        div()
            .h(px(theme::FOOTER_HEIGHT))
            .w_full()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(theme::SPACE_12))
            .px(px(theme::SPACE_16))
            // Chrome, like the sidebar/titlebar — was unpainted before,
            // showing the content plane's `bg` straight through, which
            // flattened the footer onto the same plane as the list above
            // it (SPEC §3 puts sidebar/titlebar/footer all on `surface`).
            .bg(theme.surface)
            .border_t_1()
            .border_color(theme.border)
            .text_size(px(ui::TEXT_SM))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(match &self.status {
                        Some(message) => div()
                            .min_w_0()
                            .truncate()
                            .text_color(if message.error {
                                theme.danger
                            } else {
                                theme.text_faint
                            })
                            .child(message.text.clone())
                            .into_any_element(),
                        None => render_footer_hints(&theme, content_column),
                    }),
            )
            .when_some(self.active.as_ref(), |this, repo| {
                this.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(theme::SPACE_12))
                        // "N selected" only when a real multi-selection is
                        // active (`multi_selected` is never exactly one
                        // element — see `apply_selection_set`), so a plain
                        // single-row selection keeps showing just the
                        // repo/branch chips it always has.
                        .when(self.multi_selected.len() > 1, |this| {
                            this.child(ui::meta(
                                icons::CHECK,
                                format!("{} selected", self.multi_selected.len()),
                                &theme,
                            ))
                        })
                        .child(ui::meta(icons::FOLDER, repo.name().to_string(), &theme))
                        .when_some(self.selected_branch(), |this, branch| {
                            this.child(ui::meta(icons::GIT_BRANCH, branch, &theme))
                        }),
                )
            })
    }

    // -------------------------------------------------------------
    // Detail panel
    // -------------------------------------------------------------

    /// Whether the detail panel column should be shown this frame: visible
    /// per its own toggle (folded together with the width-driven
    /// auto-collapse — see `layout::detail_panel_should_show`), and only
    /// meaningful when a row is actually selected.
    pub(super) fn show_detail_panel(&self, window: &Window) -> bool {
        self.selected.is_some()
            && layout::detail_panel_should_show(
                f32::from(window.viewport_size().width),
                self.detail_panel_visible,
                self.detail_panel_narrow_override,
            )
    }

    /// The Details/Files/Changes tab switch, invoked both by the tab bar's
    /// clicks below and by the `ShowDetailsTab`/`ShowFilesTab`/
    /// `ShowChangesTab` actions (`⌘1`/`⌘2`/`⌘3`) registered in
    /// `app::WtmApp`'s `Render` impl.
    pub(super) fn on_show_details_tab(
        &mut self,
        _: &ShowDetailsTab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_detail_tab(DetailTab::Details, cx);
    }

    /// Unlike `on_show_details_tab`, gated on `layout::wide_tabs_fit`: the
    /// wide Files/Changes panel has no narrow-width override (see that
    /// function's doc — the list column it would leave behind can go
    /// negative, not just tight), so the keyboard shortcut simply does
    /// nothing at a width where the panel wouldn't fit, the same as it
    /// would if the tab bar's own click were disabled (see
    /// `render_detail_tab`). `Render::render`'s own per-frame width sync
    /// covers the other direction — the window narrowing out from under an
    /// already-active tab.
    pub(super) fn on_show_files_tab(
        &mut self,
        _: &ShowFilesTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if layout::wide_tabs_fit(f32::from(window.viewport_size().width)) {
            self.set_detail_tab(DetailTab::Files, cx);
        }
    }

    pub(super) fn on_show_changes_tab(
        &mut self,
        _: &ShowChangesTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if layout::wide_tabs_fit(f32::from(window.viewport_size().width)) {
            self.set_detail_tab(DetailTab::Changes, cx);
        }
    }

    fn set_detail_tab(&mut self, tab: DetailTab, cx: &mut Context<Self>) {
        self.detail_tab = tab;
        cx.notify();
    }

    /// The detail panel's own current width — `detail_panel::WIDTH` for the
    /// Details tab, `detail_panel::WIDE_WIDTH` for Files/Changes, since a
    /// diff needs real room (see that constant's doc). Its own function
    /// (rather than inlined at [`render_detail_panel`]'s own call site) so
    /// [`worktree_row_card_width`](Self::worktree_row_card_width) can ask
    /// the same question the panel's own frame does, instead of a second
    /// copy of this match that could silently drift from it.
    fn detail_panel_width(&self) -> f32 {
        match self.detail_tab {
            DetailTab::Details => detail_panel::WIDTH,
            DetailTab::Files | DetailTab::Changes => detail_panel::WIDE_WIDTH,
        }
    }

    /// The detail panel: a persistent header (branch/main badge/lock),
    /// then the tab bar, then whichever tab's content is active. The outer
    /// frame's width tracks the active tab (see
    /// [`detail_panel_width`](Self::detail_panel_width)). Takes `window`
    /// only to thread it down to [`render_detail_tab_bar`], which needs the
    /// live width to know whether the Files/Changes tabs currently fit —
    /// see [`render_detail_tab`].
    pub(super) fn render_detail_panel(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(info) = self.selected.and_then(|ix| self.rows.get(ix)) else {
            return div().into_any_element();
        };
        let theme = self.chrome_theme(cx);
        let width = self.detail_panel_width();
        let worktree_path = info.path.clone();

        let content: AnyElement = match self.detail_tab {
            DetailTab::Details => {
                detail_panel::render_details(info, self.details.as_ref(), &theme).into_any_element()
            }
            DetailTab::Files => self.render_files_tab(&worktree_path, &theme, cx),
            DetailTab::Changes => self.render_changes_tab(&theme),
        };
        // Continuity (SURFACES §4's Details/Files/Changes switch): tab
        // content used to cut hard. `motion::fade_quick` keyed by the tab
        // itself (not a fixed id) is what makes this replay on every
        // switch rather than only once — a fixed id would stay mounted
        // continuously as the *panel's* content slot across every switch,
        // so gpui would never see it as newly appeared and the animation
        // state would never restart. Keying by tab means the outgoing
        // tab's content goes untouched (and gets pruned) the instant a
        // different one is selected, so switching back to it later is
        // always a fresh, real fade-in again — the correct behavior for a
        // deliberate navigation action, unlike the status-pill fade in
        // `worktree_list::render_status_pills`, which must NOT replay on
        // every unrelated re-render.
        let tab_key = match self.detail_tab {
            DetailTab::Details => "details",
            DetailTab::Files => "files",
            DetailTab::Changes => "changes",
        };
        let content_id = SharedString::from(format!("detail-tab-content-{tab_key}"));
        let content = motion::fade_quick(
            content_id,
            div().flex_1().min_h_0().min_w_0().child(content),
            cx,
        )
        .into_any_element();

        // Continuity (SPEC §5 candidate 1) — same treatment and the same
        // reasoning as `render_sidebar`'s `sidebar` binding: `w(px(width))`
        // stays instant (every layout budget that reads
        // `detail_panel_width` is correct from frame one), only opacity and
        // a slide animate.
        let panel = div()
            .w(px(width))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            // FINDINGS-2.md G1/G2: a fixed `w(px(width))` plus `flex_none()`
            // guarantees this container's own box never grows or shrinks —
            // it does *not* guarantee its children stop there. Without
            // `min_w_0()` here, `content` (Details/Files/Changes) was still
            // free to stretch past `width` following its own widest,
            // unshrunk descendant (a long commit subject, in practice), and
            // `overflow_hidden()` is what actually clips that back down to
            // the pane's real edge — the `min_w_0()` a level down in
            // `detail_panel::render_details` narrows the *available* width
            // this container hands down, but only this container's own
            // overflow behavior stops content from bleeding past it when
            // something is still wider than that. Every `content` variant
            // below already carries its own `min_w_0()`/scroll handling
            // internally; this is the pane boundary that makes those
            // guarantees actually bite.
            .min_w_0()
            .overflow_hidden()
            // A chrome pane like the sidebar, not a plate proud of one —
            // `surface_raised` (this used to read) is SPEC §3's token for
            // "opaque pills/chips proud of the panel," not for a whole
            // panel's own background; the fourth pane SURFACES' "Global"
            // section groups alongside sidebar/content reads correctly as
            // `surface`, the same chrome tone titlebar/sidebar/footer use.
            .bg(theme.surface)
            .border_l_1()
            .border_color(theme.border)
            .child(detail_panel::render_header(info, &theme))
            .child(self.render_detail_tab_bar(&theme, window, cx))
            .child(content);

        // Enters from further off the window's right edge than its resting
        // position (`start_offset_px` positive — see `motion::pane_in`'s
        // doc), mirroring the sidebar's own leading-edge slide.
        motion::pane_in("detail-panel-pane", panel, 8.0, cx).into_any_element()
    }

    /// The Details/Files/Changes switch, as a segmented control rather than
    /// three plain text buttons (SURFACES §4). The Files/Changes segments
    /// are disabled below `layout::WIDE_TABS_BREAKPOINT` — see
    /// [`render_detail_tab`]'s doc for why there's no override for this one.
    fn render_detail_tab_bar(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let wide_tabs_fit = layout::wide_tabs_fit(f32::from(window.viewport_size().width));
        // TUI parity (`tui::view`'s `"changes ({})"`): the Changes tab wears
        // the same exact-count convention once `self.details` has loaded —
        // zero extra cost, `dirty_total` is already computed for the
        // selected worktree's detail panel. Plain "Changes" while it's
        // still loading, same as every other detail-panel field that
        // degrades to its bare label until `details` arrives.
        let changes_label: SharedString = match &self.details {
            Some(details) => format!("Changes ({})", details.dirty_total).into(),
            None => "Changes".into(),
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(theme::SPACE_4))
            .px(px(theme::SPACE_12))
            .py(px(theme::SPACE_8))
            .border_b_1()
            .border_color(theme.border)
            .child(self.render_detail_tab(
                DetailTab::Details,
                "Details",
                "Details".into(),
                true,
                theme,
                cx,
            ))
            .child(self.render_detail_tab(
                DetailTab::Files,
                "Files",
                "Files".into(),
                wide_tabs_fit,
                theme,
                cx,
            ))
            .child(self.render_detail_tab(
                DetailTab::Changes,
                "Changes",
                changes_label,
                wide_tabs_fit,
                theme,
                cx,
            ))
    }

    /// One tab segment. SURFACES §4: "the selected tab carries the wash +
    /// accent underline; unselected are `text_muted`" — a wash alone would
    /// read the same as `render_sort_control`'s segments, so a 2px accent
    /// underline is what tells a tab apart from a sort option at a glance,
    /// while still keeping the accent off the fill (SPEC §3: identity/focus
    /// only, never structural). This is a tab indicator, not a selected-row
    /// mark — it stays even though `ui::row`'s own leading accent bar was
    /// removed (see that function's doc); a horizontal underline under a
    /// tab label reads as navigation, not the left-edge decoration that
    /// prompted the row change.
    ///
    /// `enabled` is `false` only for Files/Changes below
    /// `layout::WIDE_TABS_BREAKPOINT` (Details is always enabled — the
    /// panel itself is already hidden below its own, narrower breakpoint,
    /// so if this is rendering at all, Details fits). A disabled segment
    /// keeps its label (so the tab set doesn't visibly shrink — a control
    /// that's merely unreachable at this width reads very differently from
    /// one that doesn't exist) but drops its hover/click and explains why
    /// in a tooltip, rather than accepting a click into a state
    /// `Render::render`'s own width sync would just snap back out of on
    /// the very next frame.
    fn render_detail_tab(
        &self,
        tab: DetailTab,
        id_label: &'static str,
        display_label: SharedString,
        enabled: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.detail_tab == tab;
        let styled = div()
            // `.id(..)` stays keyed on the tab's static label — never the
            // dynamic `"Changes (N)"` display text — so this element's
            // identity (and gpui's per-frame diffing of it) doesn't change
            // shape every time the dirty count does.
            .id(id_label)
            .relative()
            .px(px(theme::SPACE_12))
            .py(px(theme::SPACE_6))
            .rounded(px(theme::RADIUS_CONTROL))
            .cursor_default()
            .text_size(px(ui::TEXT_SM))
            .when(active, |d| {
                d.bg(theme.element_active).text_color(theme.text)
            })
            .when(!active && enabled, |d| {
                d.text_color(theme.text_muted)
                    .hover(|s| s.bg(theme.element_hover))
            })
            .when(!enabled, |d| d.text_color(theme.text_faint))
            .child(display_label)
            .when(active, |d| {
                d.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left(px(theme::SPACE_12))
                        .right(px(theme::SPACE_12))
                        .h(px(2.0))
                        .bg(theme.accent),
                )
            });
        if enabled {
            styled
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.set_detail_tab(tab, cx);
                }))
                .into_any_element()
        } else {
            styled
                .tooltip(ui::tooltip("Widen the window to use this tab"))
                .into_any_element()
        }
    }

    /// The Files tab: a fixed-width, independently scrolling tree column,
    /// then the selected file's diff filling the rest of the panel.
    fn render_files_tab(
        &self,
        worktree_path: &Path,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tree = self.file_trees.get(worktree_path);
        let tree_panel = self.render_file_tree(tree, theme, cx);
        let diff_panel = self.render_selected_file_diff(theme);

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(
                // `.relative()` wrapper, sibling of the scrolling div — see
                // `render_changes_tab`'s identical reasoning.
                div()
                    .relative()
                    .w(px(220.0))
                    .flex_none()
                    .h_full()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .id("file-tree-scroll")
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .overflow_y_scroll()
                            .track_scroll(&self.files_tree_scroll)
                            .py(px(6.0))
                            .child(tree_panel),
                    )
                    .child(ui::scrollbar(
                        "file-tree-scrollbar",
                        &self.files_tree_scroll,
                        ui::ScrollAxis::Vertical,
                    )),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("file-diff-scroll")
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .overflow_y_scroll()
                            .track_scroll(&self.files_diff_scroll)
                            .p(px(14.0))
                            .child(diff_panel),
                    )
                    .child(ui::scrollbar(
                        "file-diff-scrollbar",
                        &self.files_diff_scroll,
                        ui::ScrollAxis::Vertical,
                    )),
            )
            .into_any_element()
    }

    /// The tree column's content: a loading/error/empty state for the root
    /// listing, or every currently visible row (`file_browser::visible_rows`)
    /// with click handling wired here — expanding/collapsing a directory
    /// row, selecting a file row — since that needs `Context<WtmApp>`,
    /// which `file_browser` itself never touches (see its module doc).
    fn render_file_tree(
        &self,
        tree: Option<&FileBrowserState>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(tree) = tree else {
            return ui::empty_hint("Loading files…", theme).into_any_element();
        };
        match tree.dir_state(Path::new("")) {
            None | Some(file_browser::DirState::Loading) => {
                ui::empty_hint("Loading files…", theme).into_any_element()
            }
            Some(file_browser::DirState::Error(e)) => {
                ui::empty_hint_error(format!("Could not list files: {e}"), theme).into_any_element()
            }
            Some(file_browser::DirState::Loaded(entries)) if entries.is_empty() => {
                ui::empty_hint("This worktree has no files.", theme).into_any_element()
            }
            Some(file_browser::DirState::Loaded(_)) => {
                let selected = tree.selected_file();
                let rows = file_browser::visible_rows(tree);
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .children(rows.into_iter().map(|row| {
                        let rel_path = row.rel_path.to_path_buf();
                        let is_dir = row.is_dir;
                        file_browser::render_row(&row, selected, theme, cx).on_click(cx.listener(
                            move |this, _, _window, cx| {
                                if is_dir {
                                    this.toggle_file_dir(rel_path.clone(), cx);
                                } else {
                                    this.select_tree_file(rel_path.clone(), cx);
                                }
                            },
                        ))
                    }))
                    .into_any_element()
            }
        }
    }

    fn render_selected_file_diff(&self, theme: &Theme) -> AnyElement {
        match &self.selected_file_diff {
            SelectedFileDiff::Unselected => {
                ui::empty_hint("Select a file to see its changes.", theme).into_any_element()
            }
            SelectedFileDiff::Loading => ui::empty_hint("Loading diff…", theme).into_any_element(),
            SelectedFileDiff::NoChanges => {
                ui::empty_hint("This file has no uncommitted changes.", theme).into_any_element()
            }
            SelectedFileDiff::Error(e) => {
                ui::empty_hint_error(format!("Could not load diff: {e}"), theme).into_any_element()
            }
            SelectedFileDiff::Changed(diff) => diff_view::render_diff(diff, theme),
        }
    }

    /// The Changes tab: every uncommitted file's diff for the selected
    /// worktree, in one scrolling column.
    fn render_changes_tab(&self, theme: &Theme) -> AnyElement {
        let content: AnyElement = match &self.changes {
            ChangesState::Loading => ui::empty_hint("Computing changes…", theme).into_any_element(),
            ChangesState::Error(e) => {
                ui::empty_hint_error(format!("Could not compute changes: {e}"), theme)
                    .into_any_element()
            }
            ChangesState::Loaded(diffs) => diff_view::render_changes(diffs, theme),
        };
        // Task 2 ("the changes panel has no scrollbar, check that"): this
        // region already scrolled (`.overflow_y_scroll()` below), gpui
        // 0.2.2 just never painted anything to show it. `.relative()`
        // wrapper + sibling overlay, same reasoning as `render_list`'s.
        let edges = ui::scroll_edges(
            f32::from(self.changes_scroll.offset().y),
            f32::from(self.changes_scroll.max_offset().height),
        );
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("changes-scroll")
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&self.changes_scroll)
                    .p(px(14.0))
                    .child(content),
            )
            .when(edges.leading, |this| {
                this.child(ui::scroll_fade_top(theme.surface, theme::SPACE_24))
            })
            .when(edges.trailing, |this| {
                this.child(ui::scroll_fade_bottom(theme.surface, theme::SPACE_24))
            })
            .child(ui::scrollbar(
                "changes-scrollbar",
                &self.changes_scroll,
                ui::ScrollAxis::Vertical,
            ))
            .into_any_element()
    }
}

/// Character budget for a sidebar repo row's path — see
/// `worktree_list::ROW_PATH_MAX_CHARS`'s doc for why this is an
/// approximate, always-fits-the-narrow-case budget rather than exact pixel
/// arithmetic, and `detail_panel::LABEL_WIDTH`'s doc for why any of this is
/// necessary instead of gpui's own `.truncate()`. `SIDEBAR_WIDTH` (248) is
/// fixed, so in principle this could be exact the way the detail panel's
/// fact values are — left approximate anyway since the icon glyph beside
/// it isn't a clean constant the way `LABEL_WIDTH`/`COMMIT_SHA_WIDTH` are,
/// and an approximate budget already comfortably fits this narrower,
/// simpler row.
const SIDEBAR_PATH_MAX_CHARS: usize = 28;

/// Where a repository lives, home-relative and without the repo's own
/// directory name — the sidebar already shows that on the line above, and
/// what disambiguates two repos with the same name is the folder holding
/// them. Capped at [`SIDEBAR_PATH_MAX_CHARS`] with a leading ellipsis (the
/// tail — the folder actually holding the repo — is what disambiguates, so
/// it's what has to survive), via the same `truncate_path_tail` mechanism
/// `detail_panel`/`worktree_list` use for their own paths.
fn parent_label(path: &std::path::Path) -> String {
    let parent = path.parent().unwrap_or(path).display().to_string();
    let home_relative = match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && parent.starts_with(&home) => {
            format!("~{}", &parent[home.len()..])
        }
        _ => parent,
    };
    detail_panel::truncate_path_tail(&home_relative, SIDEBAR_PATH_MAX_CHARS)
}

/// Missing/prunable worktrees the active repository has right now, using the
/// same baseline `dialogs::PruneState::new()` starts with (`merged`/`gone`
/// both off) — see `render_list`'s use of this next to the toolbar's Prune…
/// button. A free function over `(repo, rows)` rather than a `&self` method
/// so it is directly testable against a synthetic repo/rows, the same shape
/// `crate::dialogs`'s own prune tests already use, rather than only
/// reachable through a live `WtmApp`.
fn prunable_count(repo: &OpenRepo, rows: &[WorktreeInfo]) -> usize {
    data::prune_candidates(repo, rows.to_vec(), false, false).len()
}

/// The window controls Linux draws in its own title bar when the compositor
/// grants client-side decorations — minimize, maximize/restore, and close,
/// in the right-side order GNOME and KDE both use (`render_titlebar` places
/// this last, after every other title-bar button). macOS never renders
/// this: it always keeps `Decorations::Server` and its real traffic lights
/// instead — see `render_titlebar`'s `csd` guard.
fn render_window_controls(theme: &Theme, cx: &mut Context<WtmApp>) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(2.0))
        .child(
            window_control_button("win-minimize", theme)
                .child(minimize_glyph(theme))
                .on_click(|_, window, _cx| window.minimize_window()),
        )
        .child(
            window_control_button("win-maximize", theme)
                .child(maximize_glyph(theme))
                .on_click(|_, window, _cx| window.zoom_window()),
        )
        .child(
            window_control_button("win-close", theme)
                .child(ui::icon(icons::CLOSE, 12.0, theme.text_faint))
                .on_click(cx.listener(|_this, _, window, cx| {
                    // A client-side close button is not the OS-level close
                    // gesture `main.rs`'s `on_window_should_close` is
                    // registered against (that hook only fires for a
                    // platform-originated close request), so calling
                    // `remove_window` alone would skip it — and with it,
                    // the window-frame save that hook exists to do. This
                    // does that save by hand instead, reusing the exact
                    // function the real close path calls.
                    let view = cx.entity();
                    crate::save_prefs_with_window_frame(&view, window, cx);
                    window.remove_window();
                })),
        )
}

/// The base of a Linux window-control button: the same hover square
/// `ui::icon_button` uses (`theme::ICON_BUTTON_SIZE`), but built here
/// directly rather than through it, since minimize/maximize need a
/// caller-supplied glyph in place of an svg icon — see
/// `minimize_glyph`/`maximize_glyph` below for why.
fn window_control_button(id: &'static str, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(theme::ICON_BUTTON_SIZE))
        .h(px(theme::ICON_BUTTON_SIZE))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(theme::RADIUS_CONTROL))
        .cursor_default()
        .hover(|this| this.bg(theme.element_hover))
}

/// A minimize glyph: a single horizontal line, the shape every desktop
/// environment uses for it. Composed from a plain `div()` rather than an
/// svg asset — `assets.rs` is owned elsewhere and not extended for this
/// task, and it has nothing shaped like this to begin with.
fn minimize_glyph(theme: &Theme) -> impl IntoElement {
    div().w(px(10.0)).h(px(1.0)).bg(theme.text_faint)
}

/// A maximize/restore glyph: a small square outline, composed the same way
/// `minimize_glyph` is.
fn maximize_glyph(theme: &Theme) -> impl IntoElement {
    div()
        .w(px(9.0))
        .h(px(9.0))
        .border_1()
        .border_color(theme.text_faint)
}

/// The footer's default (no active status message) hint line: the highest-
/// value keybindings as [`ui::kbd`] chips rather than plain text (SPEC §1's
/// `kbd` vocabulary), so a shortcut named in the footer looks like the same
/// shortcut everywhere else in the app instead of a bare string.
fn render_footer_hints(theme: &Theme, content_column: f32) -> AnyElement {
    // FINDINGS-2.md G1: this row used to hard-clip its trailing hints with
    // no ellipsis at 900×600 (with the detail panel open — plain sidebar
    // widths never got this narrow). An earlier attempt made the last hint
    // `min_w_0()`/`.truncate()` so it would shrink and ellipsize instead —
    // that made things *worse* (gpui 0.2.2's text-measurement caching bug,
    // documented once at `detail_panel::LABEL_WIDTH`, meant it collapsed to
    // 2-3 characters with no ellipsis and a large unused gap, rather than
    // clipping cleanly). Every hint here is a short, fixed,
    // known-at-compile-time string, not user content, so — unlike the
    // genuinely-long, unbounded strings elsewhere in this app (paths,
    // commit subjects) that need one of the truncation strategies
    // `detail_panel::LABEL_WIDTH` explains — there is no per-glyph
    // ellipsis story to lean on here. The actual fix: drop whole hints by
    // priority (`layout::FooterHints`) once the live content column
    // reports there isn't room for all of them, rather than shrinking any
    // one of them.
    let row = div()
        .flex()
        .items_center()
        .gap(px(theme::SPACE_6))
        .text_color(theme.text_ghost);
    use layout::FooterHints;
    match FooterHints::for_content_column(content_column) {
        FooterHints::All => row
            .child(ui::kbd("↑↓", theme))
            .child("select")
            .child(ui::kbd("⏎", theme))
            .child("open in editor")
            .child(ui::kbd("⌘R", theme))
            .child("reload")
            .into_any_element(),
        FooterHints::Core => row
            .child(ui::kbd("↑↓", theme))
            .child("select")
            .child(ui::kbd("⏎", theme))
            .child("open in editor")
            .into_any_element(),
        FooterHints::Minimal => row
            .child(ui::kbd("↑↓", theme))
            .child("select")
            .into_any_element(),
        FooterHints::None => div().into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use wtm::config::Config;
    use wtm::model::WorktreeStatus;
    use wtm::repo::RepoContext;

    use super::*;

    fn fake_repo(protected_branches: Vec<String>) -> OpenRepo {
        OpenRepo {
            ctx: RepoContext {
                main_root: PathBuf::from("/tmp/repo"),
                git_dir: PathBuf::from("/tmp/repo/.git"),
                repo_name: "repo".to_string(),
            },
            config: Config {
                prune: wtm::config::PruneConfig { protected_branches },
                ..Config::default()
            },
        }
    }

    fn worktree(name: &str, is_main: bool, missing: bool, prunable: bool) -> WorktreeInfo {
        WorktreeInfo {
            name: name.to_string(),
            path: PathBuf::from(format!("/tmp/{name}")),
            branch: Some(name.to_string()),
            head: None,
            is_main,
            is_missing: missing,
            is_locked: false,
            is_prunable: prunable,
            status: Some(WorktreeStatus {
                dirty: false,
                dirty_count: 0,
                ahead: None,
                behind: None,
                upstream_gone: false,
                merged: false,
            }),
        }
    }

    #[test]
    fn prunable_count_is_zero_with_nothing_stale() {
        let repo = fake_repo(vec![]);
        let rows = vec![
            worktree("main", true, false, false),
            worktree("feature", false, false, false),
        ];
        assert_eq!(prunable_count(&repo, &rows), 0);
    }

    #[test]
    fn prunable_count_matches_missing_and_prunable_rows() {
        let repo = fake_repo(vec![]);
        let rows = vec![
            worktree("main", true, false, false),
            worktree("gone-dir", false, true, false),
            worktree("stale", false, false, true),
            worktree("fine", false, false, false),
        ];
        assert_eq!(prunable_count(&repo, &rows), 2);
    }

    #[test]
    fn prunable_count_never_counts_the_main_worktree() {
        // The main worktree can never be missing or prunable in practice,
        // but `candidates` itself refuses it unconditionally — assert that
        // guarantee holds through this free function too.
        let repo = fake_repo(vec![]);
        let rows = vec![worktree("main", true, true, true)];
        assert_eq!(prunable_count(&repo, &rows), 0);
    }

    #[test]
    fn prunable_count_skips_protected_branches() {
        let repo = fake_repo(vec!["release".to_string()]);
        let rows = vec![worktree("release", false, true, true)];
        assert_eq!(prunable_count(&repo, &rows), 0);
    }
}

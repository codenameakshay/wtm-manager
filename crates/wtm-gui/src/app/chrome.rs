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

impl WtmApp {
    /// The sidebar: window controls clearance, actions, then the repo list.
    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let active_path = self.active.as_ref().map(|r| r.path().to_path_buf());

        div()
            .w(px(248.0))
            .h_full()
            .flex()
            .flex_none()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            // The title bar is transparent, so the sidebar starts under the
            // traffic lights and has to leave room for them.
            .child(div().h(px(ui::TITLEBAR_HEIGHT)).flex_none())
            .child(
                div()
                    .px(px(8.0))
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
                    ),
            )
            .child(div().h(px(10.0)).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .px(px(8.0))
                    .child(
                        // `ui::section_header` has no slot for a trailing
                        // action and `ui.rs` is not owned by this task to add
                        // one, so its exact height/inset/text styling is
                        // reproduced here around a real affordance instead —
                        // the user's own complaint was "how do I add more
                        // repos (no plus button)?"; this is that button.
                        div()
                            .h(px(28.0))
                            .pl(px(8.0))
                            .pr(px(4.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(theme.text_tertiary)
                                    .child("Repositories"),
                            )
                            .child(
                                ui::icon_button("add-repository", icons::PLUS, &theme).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.on_add_repository(&AddRepository, window, cx);
                                    }),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(1.0))
                            .children(self.repos.iter().map(|entry| {
                                self.render_repo_row(entry, active_path.as_deref(), &theme, cx)
                            }))
                            .when(self.repos.is_empty(), |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(6.0))
                                        .px(px(8.0))
                                        .py(px(6.0))
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .text_color(theme.text_ghost)
                                                .child("No repositories yet."),
                                        )
                                        .child(
                                            ui::action_row(
                                                "add-repository-empty",
                                                icons::PLUS,
                                                "Add Repository…",
                                                Some("⌘⇧O"),
                                                &theme,
                                            )
                                            .on_click(
                                                cx.listener(|this, _, window, cx| {
                                                    this.on_add_repository(
                                                        &AddRepository,
                                                        window,
                                                        cx,
                                                    );
                                                }),
                                            ),
                                        ),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .h(px(40.0))
                    .px(px(10.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .child(
                        ui::icon_button("settings", icons::SETTINGS, &theme).on_click(cx.listener(
                            |this, _, window, cx| {
                                this.on_open_settings(&OpenSettings, window, cx);
                            },
                        )),
                    ),
            )
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
        .gap(px(3.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .line_height(px(18.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.0))
                        .text_color(if missing {
                            theme.text_tertiary
                        } else {
                            theme.text
                        })
                        .child(entry.name.clone()),
                )
                .when(missing, |this| {
                    this.child(ui::icon(icons::WARNING, 12.0, theme.warning))
                }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .text_size(px(11.5))
                .line_height(px(15.0))
                .child(ui::meta(icons::FOLDER, parent_label(&entry.path), theme)),
        )
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
        let theme = Theme::of(cx);
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
            .gap(px(8.0))
            .px(px(10.0))
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
                ui::icon_button("toggle-sidebar", icons::PANEL_LEFT, &theme).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.on_toggle_sidebar(&ToggleSidebar, window, cx);
                    },
                )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.0))
                            .text_color(theme.text)
                            .child(title),
                    )
                    .when_some(self.selected_branch(), |this, branch| {
                        this.child(
                            div()
                                .flex()
                                .min_w_0()
                                .items_center()
                                .gap(px(6.0))
                                .text_size(px(12.5))
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
                ui::icon_button("open-selected", icons::OPEN_EXTERNAL, &theme).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.on_open_selected(&OpenSelected, window, cx);
                    }),
                ),
            )
            .child(
                ui::icon_button("reload", icons::REFRESH, &theme).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.on_reload(&Reload, window, cx);
                    },
                )),
            )
            .child(
                // No dedicated "inspector"/"panel-right" icon is embedded in
                // `assets.rs` (owned elsewhere, not extended for this task);
                // reusing the sidebar's own panel glyph is the closest
                // available fit rather than adding a mismatched one.
                ui::icon_button("toggle-detail-panel", icons::PANEL_LEFT, &theme).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.on_toggle_detail_panel(&ToggleDetailPanel, window, cx);
                    }),
                ),
            )
            .when(csd, |this| this.child(render_window_controls(&theme, cx)))
    }

    fn selected_branch(&self) -> Option<String> {
        let info = self.rows.get(self.selected?)?;
        Some(info.display_name().to_string())
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
            return div()
                .flex_1()
                .child(worktree_list::render_no_repo(cx))
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
            let theme = Theme::of(cx);
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Right, empty_space_menu)
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(theme.text_secondary)
                        .child("Listing worktrees…"),
                )
                .into_any_element();
        }
        if self.rows.is_empty() && !self.loading {
            return div()
                .flex_1()
                .child(worktree_list::render_empty(cx))
                .on_mouse_down(MouseButton::Right, empty_space_menu)
                .into_any_element();
        }

        let theme = Theme::of(cx);
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
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(1040.0))
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        // The count on the left grows to fill the row; the
                        // toolbar buttons, filter field, and its clear
                        // button stay fixed-size on the right — real,
                        // labeled affordances for New Worktree and Prune
                        // rather than actions only a shortcut table names,
                        // and a persistent, always-discoverable search box
                        // rather than one hidden until ⌘F.
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .px(px(16.0))
                            .pb(px(8.0))
                            .child(div().flex_1().min_w_0().child(worktree_list::render_header(
                                shown,
                                total,
                                self.loading,
                                cx,
                            )))
                            .child(
                                toolbar_button(
                                    "toolbar-new-worktree",
                                    icons::PLUS,
                                    "New Worktree",
                                    &theme,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.on_new_worktree(&NewWorktree, window, cx);
                                    },
                                )),
                            )
                            .child(
                                // Label and appearance both change while a
                                // fetch is running, but the real guard
                                // against a second concurrent `git fetch` is
                                // `on_fetch_remote`'s own `self.fetching`
                                // check — this is only the visible half of
                                // that promise.
                                toolbar_button(
                                    "toolbar-fetch",
                                    icons::REFRESH,
                                    if self.fetching {
                                        "Fetching…"
                                    } else {
                                        "Fetch"
                                    },
                                    &theme,
                                )
                                .when(self.fetching, |this| this.opacity(0.6))
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.on_fetch_remote(&FetchRemote, window, cx);
                                    },
                                )),
                            )
                            .child(
                                // Opens the same confirm-with-toggles dialog
                                // as the shortcut/menu path always has —
                                // pruning without a confirmation step would
                                // be destructive, so this button is a
                                // discoverable door to that dialog, not a
                                // way around it. The count in the label is
                                // what lets a user see there is something to
                                // clean without opening anything.
                                toolbar_button("toolbar-prune", icons::TRASH, prune_label, &theme)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_prune_repo(&PruneRepo, window, cx);
                                    })),
                            )
                            .child(self.render_sort_control(&theme, cx))
                            .child(
                                div()
                                    .w(px(200.0))
                                    .flex_none()
                                    .child(self.filter_input.clone()),
                            )
                            .when(filter_active, |this| {
                                this.child(
                                    ui::icon_button("clear-filter", icons::CLOSE, &theme).on_click(
                                        cx.listener(|this, _, window, cx| {
                                            this.clear_filter(window, cx)
                                        }),
                                    ),
                                )
                            }),
                    )
                    .when(multi_count > 1, |this| {
                        this.child(self.render_selection_bar(multi_count, &theme, cx))
                    })
                    .child(
                        uniform_list(
                            "worktrees",
                            shown,
                            cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                                let visible = this.visible_row_indices(cx);
                                let theme = Theme::of(cx);
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
                                let force_checkbox_visible = !this.multi_selected.is_empty();
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
                                        let group_name = SharedString::from(format!("wt-row-{ix}"));
                                        div()
                                            .px(px(8.0))
                                            .pb(px(2.0))
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
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
                            }),
                        )
                        .flex_1()
                        .px(px(8.0))
                        .on_mouse_down(MouseButton::Right, empty_space_menu),
                    ),
            )
            .into_any_element()
    }

    /// The list toolbar's sort-mode switch: a compact three-way segmented
    /// control (Name / Recent / Status) reusing `render_detail_tab`'s
    /// active/inactive visual language at a smaller size. Clicking a
    /// segment goes through `selection::set_sort_mode`, which re-sorts
    /// immediately and keeps the current selection on the same worktree —
    /// this control itself has nothing to do about that, or about the main
    /// worktree staying pinned first (both are `worktree_list::sort_rows`'s
    /// own guarantees).
    fn render_sort_control(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.0))
            .p(px(2.0))
            .rounded(px(ui::RADIUS))
            .bg(theme.item_wash)
            .children(
                worktree_list::SortMode::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(idx, mode)| {
                        let active = self.sort_mode == mode;
                        div()
                            .id(("sort-mode", idx))
                            .px(px(8.0))
                            .h(px(24.0))
                            .flex()
                            .items_center()
                            .rounded(px(ui::RADIUS))
                            .cursor_default()
                            .text_size(px(12.0))
                            .when(active, |d| d.bg(theme.item_selected).text_color(theme.text))
                            .when(!active, |d| {
                                d.text_color(theme.text_tertiary)
                                    .hover(|s| s.bg(theme.item_selected))
                            })
                            .child(worktree_list::sort_mode_label(mode))
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.set_sort_mode(mode, cx);
                            }))
                    }),
            )
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
            .w(px(15.0))
            .h(px(15.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .cursor_default()
            .border_1()
            .border_color(if checked {
                theme.accent
            } else {
                theme.border_strong
            })
            .bg(if checked {
                theme.accent
            } else {
                gpui::transparent_black()
            })
            .when(!checked && !force_visible, |this| this.opacity(0.0))
            .group_hover(group_name, |style| style.opacity(1.0))
            .when(checked, |this| {
                this.child(ui::icon(icons::CHECK, 10.0, theme.canvas))
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
            .gap(px(10.0))
            .px(px(16.0))
            .pb(px(8.0))
            .child(ui::meta(icons::CHECK, format!("{count} selected"), theme))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .text_size(px(11.0))
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
    /// right, in the spirit of a status line that never shouts.
    pub(super) fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);

        div()
            .h(px(34.0))
            .w_full()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(12.0))
            .px(px(16.0))
            .border_t_1()
            .border_color(theme.border)
            .text_size(px(11.5))
            .child(match &self.status {
                Some(message) => div()
                    .min_w_0()
                    .truncate()
                    .text_color(if message.error {
                        theme.danger
                    } else {
                        theme.text_tertiary
                    })
                    .child(message.text.clone()),
                None => div()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.text_ghost)
                    .child("↑↓ select · ⏎ open in editor · ⌘R reload"),
            })
            .when_some(self.active.as_ref(), |this, repo| {
                this.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(12.0))
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
    /// per its own toggle, and only meaningful when a row is actually
    /// selected.
    pub(super) fn show_detail_panel(&self) -> bool {
        self.detail_panel_visible && self.selected.is_some()
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

    pub(super) fn on_show_files_tab(
        &mut self,
        _: &ShowFilesTab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_detail_tab(DetailTab::Files, cx);
    }

    pub(super) fn on_show_changes_tab(
        &mut self,
        _: &ShowChangesTab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_detail_tab(DetailTab::Changes, cx);
    }

    fn set_detail_tab(&mut self, tab: DetailTab, cx: &mut Context<Self>) {
        self.detail_tab = tab;
        cx.notify();
    }

    /// The detail panel: a persistent header (branch/main badge/lock),
    /// then the tab bar, then whichever tab's content is active. The outer
    /// frame's width tracks the active tab — `detail_panel::WIDTH` for
    /// Details, `detail_panel::WIDE_WIDTH` for Files/Changes, since a diff
    /// needs real room (see that constant's doc).
    pub(super) fn render_detail_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(info) = self.selected.and_then(|ix| self.rows.get(ix)) else {
            return div().into_any_element();
        };
        let theme = Theme::of(cx);
        let width = match self.detail_tab {
            DetailTab::Details => detail_panel::WIDTH,
            DetailTab::Files | DetailTab::Changes => detail_panel::WIDE_WIDTH,
        };
        let worktree_path = info.path.clone();

        let content: AnyElement = match self.detail_tab {
            DetailTab::Details => {
                detail_panel::render_details(info, self.details.as_ref(), &theme).into_any_element()
            }
            DetailTab::Files => self.render_files_tab(&worktree_path, &theme, cx),
            DetailTab::Changes => self.render_changes_tab(&theme),
        };

        div()
            .w(px(width))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme.raised)
            .border_l_1()
            .border_color(theme.border)
            .child(detail_panel::render_header(info, &theme))
            .child(self.render_detail_tab_bar(&theme, cx))
            .child(content)
            .into_any_element()
    }

    fn render_detail_tab_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(12.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(theme.border)
            .child(self.render_detail_tab(DetailTab::Details, "Details", theme, cx))
            .child(self.render_detail_tab(DetailTab::Files, "Files", theme, cx))
            .child(self.render_detail_tab(DetailTab::Changes, "Changes", theme, cx))
    }

    fn render_detail_tab(
        &self,
        tab: DetailTab,
        label: &'static str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.detail_tab == tab;
        div()
            .id(label)
            .px(px(10.0))
            .py(px(5.0))
            .rounded(px(ui::RADIUS))
            .cursor_default()
            .text_size(px(12.0))
            .when(active, |d| d.bg(theme.item_selected).text_color(theme.text))
            .when(!active, |d| {
                d.text_color(theme.text_tertiary)
                    .hover(|s| s.bg(theme.item_wash))
            })
            .child(label)
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.set_detail_tab(tab, cx);
            }))
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
                div()
                    .id("file-tree-scroll")
                    .w(px(220.0))
                    .flex_none()
                    .h_full()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .border_r_1()
                    .border_color(theme.border)
                    .py(px(6.0))
                    .child(tree_panel),
            )
            .child(
                div()
                    .id("file-diff-scroll")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .child(diff_panel),
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
                ui::empty_hint(format!("Could not list files: {e}"), theme).into_any_element()
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
                    .children(rows.into_iter().map(|row| {
                        let rel_path = row.rel_path.to_path_buf();
                        let is_dir = row.is_dir;
                        file_browser::render_row(&row, selected, theme).on_click(cx.listener(
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
                ui::empty_hint(format!("Could not load diff: {e}"), theme).into_any_element()
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
                ui::empty_hint(format!("Could not compute changes: {e}"), theme).into_any_element()
            }
            ChangesState::Loaded(diffs) => diff_view::render_changes(diffs, theme),
        };
        div()
            .id("changes-scroll")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .p(px(14.0))
            .child(content)
            .into_any_element()
    }
}

/// Where a repository lives, home-relative and without the repo's own
/// directory name — the sidebar already shows that on the line above, and
/// what disambiguates two repos with the same name is the folder holding them.
fn parent_label(path: &std::path::Path) -> String {
    let parent = path.parent().unwrap_or(path).display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && parent.starts_with(&home) => {
            format!("~{}", &parent[home.len()..])
        }
        _ => parent,
    }
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

/// A compact icon+label toolbar button. `ui::button` has no icon slot and
/// `ui.rs` is not owned by this task to add one, so this reuses its exact
/// visual language instead — the `Secondary` variant's `item_wash`/
/// `item_selected` wash and `ui::RADIUS`, at the same 28px height — for the
/// list toolbar's New Worktree / Prune… actions.
fn toolbar_button(
    id: impl Into<gpui::ElementId>,
    icon_path: &'static str,
    label: impl Into<SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .h(px(28.0))
        .px(px(10.0))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.0))
        .rounded(px(ui::RADIUS))
        .cursor_default()
        .bg(theme.item_wash)
        .hover(|this| this.bg(theme.item_selected))
        .active(|this| this.bg(theme.item_selected))
        .child(ui::icon(icon_path, 13.0, theme.text_secondary))
        .child(
            div()
                .text_size(px(12.5))
                .text_color(theme.text)
                .child(label.into()),
        )
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
                .child(ui::icon(icons::CLOSE, 12.0, theme.text_tertiary))
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

/// The base of a Linux window-control button: the same 26×26 hover square
/// `ui::icon_button` uses, but built here directly rather than through it,
/// since minimize/maximize need a caller-supplied glyph in place of an svg
/// icon — see `minimize_glyph`/`maximize_glyph` below for why.
fn window_control_button(id: &'static str, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(26.0))
        .h(px(26.0))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor_default()
        .hover(|this| this.bg(theme.item_wash))
}

/// A minimize glyph: a single horizontal line, the shape every desktop
/// environment uses for it. Composed from a plain `div()` rather than an
/// svg asset — `assets.rs` is owned elsewhere and not extended for this
/// task, and it has nothing shaped like this to begin with.
fn minimize_glyph(theme: &Theme) -> impl IntoElement {
    div().w(px(10.0)).h(px(1.0)).bg(theme.text_tertiary)
}

/// A maximize/restore glyph: a small square outline, composed the same way
/// `minimize_glyph` is.
fn maximize_glyph(theme: &Theme) -> impl IntoElement {
    div()
        .w(px(9.0))
        .h(px(9.0))
        .border_1()
        .border_color(theme.text_tertiary)
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

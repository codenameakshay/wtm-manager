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
                    .child(ui::action_row(
                        "search",
                        icons::SEARCH,
                        "Search",
                        Some("⌘K"),
                        &theme,
                    )),
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
                    .child(ui::section_header("Repositories", &theme))
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
                                        .px(px(8.0))
                                        .py(px(6.0))
                                        .text_size(px(12.0))
                                        .text_color(theme.text_ghost)
                                        .child("Run `wtm` inside a repository to add it here."),
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
    /// active repository, and the actions that apply to it.
    pub(super) fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let title = self
            .active
            .as_ref()
            .map(|repo| repo.name().to_string())
            .unwrap_or_else(|| "wtm".to_string());

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
            .when(!self.sidebar_visible, |this| {
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
    }

    fn selected_branch(&self) -> Option<String> {
        let info = self.rows.get(self.selected?)?;
        Some(info.display_name().to_string())
    }

    pub(super) fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.active.is_none() {
            return div()
                .flex_1()
                .child(worktree_list::render_no_repo(cx))
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
                .into_any_element();
        }

        let theme = Theme::of(cx);
        let visible = self.visible_row_indices(cx);
        let shown = visible.len();
        let total = self.rows.len();
        let filter_active = !self.filter_input.read(cx).value().trim().is_empty();

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
                        // filter field and its clear button stay fixed-size
                        // on the right — a persistent, always-discoverable
                        // search box rather than one hidden until ⌘F, which
                        // is the "your call" this task leaves open.
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
                    .child(
                        uniform_list(
                            "worktrees",
                            shown,
                            cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                                let visible = this.visible_row_indices(cx);
                                range
                                    .map(|display_ix| {
                                        let ix = visible[display_ix];
                                        let selected = this.is_row_selected(ix);
                                        div().px(px(8.0)).pb(px(2.0)).child(
                                            worktree_list::render_row(
                                                &this.rows[ix],
                                                ix,
                                                selected,
                                                this.awaiting_status,
                                                cx,
                                            )
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
                                            ))
                                            .on_mouse_down(
                                                MouseButton::Right,
                                                cx.listener(
                                                    move |this, event: &MouseDownEvent, _, cx| {
                                                        this.open_worktree_context_menu(
                                                            ix,
                                                            event.position,
                                                            cx,
                                                        );
                                                    },
                                                ),
                                            ),
                                        )
                                    })
                                    .collect()
                            }),
                        )
                        .flex_1()
                        .px(px(8.0)),
                    ),
            )
            .into_any_element()
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

    pub(super) fn render_detail_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(info) = self.selected.and_then(|ix| self.rows.get(ix)) else {
            return div().into_any_element();
        };
        detail_panel::render(info, self.details.as_ref(), cx).into_any_element()
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

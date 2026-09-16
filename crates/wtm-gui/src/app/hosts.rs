//! Remote hosts: the sidebar's Hosts section, the pane that lists a host's
//! repositories and worktrees with their disk usage (shown in place of the
//! worktree list), and the Add Host and removal confirmation modals.
//!
//! Every host operation is one blocking ssh round trip in `wtm::remote`, so
//! all of them run on the background executor. A shown host and an open
//! repository are mutually exclusive: `select_host` closes the repository,
//! and `begin_activate_repo` closes the host. Scans share `generation` with
//! repository listings, so switching either way drops the other's
//! in-flight results.

use wtm::commands::prune;
use wtm::remote::RemoteRepo;

use super::chrome::{LIST_MAX_WIDTH, ROW_CHECKBOX_SIZE, SIDEBAR_PATH_MAX_CHARS};
use super::dialog_forms::{destructive_count_line, labeled_field, uncommitted_changes_warning};
use super::*;
use crate::detail_panel::{truncate_path_tail, truncate_tail};

/// A host shown in place of the worktree list.
pub(super) struct HostView {
    pub(super) host: Host,
    /// `None` until the first scan lands.
    pub(super) repos: Option<Vec<RemoteRepo>>,
    /// The latest listing came from the sized pass.
    pub(super) sized: bool,
    pub(super) scanning: bool,
    /// A removal is running on this host.
    pub(super) busy: bool,
    pub(super) error: Option<String>,
    /// Selected worktrees by path, so a rescan's new order keeps them.
    pub(super) selected: BTreeSet<PathBuf>,
    scroll: ScrollHandle,
}

impl HostView {
    fn new(host: Host) -> Self {
        Self {
            host,
            repos: None,
            sized: false,
            scanning: false,
            busy: false,
            error: None,
            selected: BTreeSet::new(),
            scroll: ScrollHandle::new(),
        }
    }
}

/// The host modals. Its own field beside `dialog`, like `run_command`.
pub(super) enum HostDialog {
    Add(AddHostState),
    Confirm(HostConfirmState),
}

pub(super) struct AddHostState {
    pub(super) name: Entity<TextInput>,
    pub(super) destination: Entity<TextInput>,
    pub(super) roots: Entity<TextInput>,
    pub(super) error: Option<String>,
    _subs: Vec<Subscription>,
}

impl AddHostState {
    fn new(window: &mut Window, cx: &mut Context<WtmApp>) -> Self {
        let (name, name_sub) = host_field("vps", window, cx);
        let (destination, destination_sub) = host_field("vps or ubuntu@1.2.3.4", window, cx);
        let (roots, roots_sub) = host_field("~ (default)", window, cx);
        Self {
            name,
            destination,
            roots,
            error: None,
            _subs: vec![name_sub, destination_sub, roots_sub],
        }
    }
}

fn host_field(
    placeholder: &'static str,
    window: &mut Window,
    cx: &mut Context<WtmApp>,
) -> (Entity<TextInput>, Subscription) {
    let input = cx.new(|cx| TextInput::new(placeholder, cx));
    let sub = cx.subscribe_in(&input, window, {
        move |app: &mut WtmApp, _input, event, window, cx| match event {
            InputEvent::Submit => app.submit_add_host(window, cx),
            InputEvent::Cancel => app.close_dialog(window, cx),
            // The helper line under the fields repeats the destination.
            InputEvent::Changed => cx.notify(),
        }
    });
    (input, sub)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ConfirmKind {
    Remove,
    CleanUp,
}

/// A repository path and its removal candidates with their known sizes.
pub(super) type RemovalGroup = (PathBuf, Vec<(PruneCandidate, Option<u64>)>);

/// The confirmation shared by Remove Selected and a repository's Clean Up.
pub(super) struct HostConfirmState {
    pub(super) kind: ConfirmKind,
    pub(super) groups: Vec<RemovalGroup>,
    pub(super) force: bool,
}

/// What a background removal did, across every repository it touched.
#[derive(Default)]
struct HostRemoveReport {
    removed: usize,
    freed: u64,
    /// One line per worktree that was not removed, or was removed with a
    /// note (such as a kept branch), naming it.
    failures: Vec<String>,
}

fn protected_branches() -> Vec<String> {
    wtm::config::load_global()
        .map(|c| c.prune.protected_branches)
        .unwrap_or_else(|_| wtm::config::PruneConfig::default().protected_branches)
}

/// A rescan's unsized pass would otherwise re-sort every repository by path
/// for the ~20s the sized pass takes, then jump back; keep the previous
/// sizes until fresh ones land.
fn carry_sizes(repos: &mut [RemoteRepo], previous: &[RemoteRepo]) {
    let known: HashMap<&Path, u64> = previous
        .iter()
        .flat_map(|r| &r.worktrees)
        .filter_map(|w| Some((w.info.path.as_path(), w.size_bytes?)))
        .collect();
    for w in repos.iter_mut().flat_map(|r| r.worktrees.iter_mut()) {
        if w.size_bytes.is_none() {
            w.size_bytes = known.get(w.info.path.as_path()).copied();
        }
    }
}

fn with_sizes(
    repo: &RemoteRepo,
    candidates: Vec<PruneCandidate>,
) -> Vec<(PruneCandidate, Option<u64>)> {
    candidates
        .into_iter()
        .map(|c| {
            let size = repo
                .worktrees
                .iter()
                .find(|w| w.info.path == c.info.path)
                .and_then(|w| w.size_bytes);
            (c, size)
        })
        .collect()
}

/// One `remove_worktrees` call per repository. A repository whose ssh call
/// fails becomes a failure line; the rest still run.
fn remove_groups(host: &Host, groups: &[RemovalGroup], force: bool) -> HostRemoveReport {
    let mut report = HostRemoveReport::default();
    for (repo_path, entries) in groups {
        let targets: Vec<PruneCandidate> = entries.iter().map(|(c, _)| c.clone()).collect();
        let outcomes = match remote::remove_worktrees(host, repo_path, &targets, force) {
            Ok(outcomes) => outcomes,
            Err(e) => {
                report
                    .failures
                    .push(format!("{}: {e}", repo_path.display()));
                continue;
            }
        };
        for outcome in outcomes {
            let entry = entries.iter().find(|(c, _)| c.info.path == outcome.path);
            let name = entry.map_or_else(
                || outcome.path.display().to_string(),
                |(c, _)| c.info.display_name().to_string(),
            );
            if outcome.removed {
                report.removed += 1;
                report.freed += entry.and_then(|(_, size)| *size).unwrap_or(0);
            }
            match (outcome.removed, outcome.message) {
                (_, Some(message)) => report.failures.push(format!("{name}: {message}")),
                (false, None) => report.failures.push(format!("{name}: not removed")),
                (true, None) => {}
            }
        }
    }
    report
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Text already truncated in Rust (gpui 0.2.2's `.truncate()` is broken, see
/// `detail_panel::LABEL_WIDTH`). It never shrinks, so its natural width is
/// the right one; a width from `ui::CHAR_WIDTH_APPROX`, a small-monospace
/// estimate, would clip larger or bold text.
fn text_box(text: String) -> Div {
    div().flex_none().whitespace_nowrap().child(text)
}

/// "Clean Up" at `TEXT_BASE` plus `ui::button`'s `SPACE_12` padding.
const CLEANUP_BUTTON_WIDTH: f32 = 8.0 * ui::CHAR_WIDTH_APPROX + theme::SPACE_12 * 2.0;

impl WtmApp {
    // -------------------------------------------------------------
    // Selection and scanning
    // -------------------------------------------------------------

    /// Show `host` in place of the worktree list and scan it.
    pub(super) fn select_host(&mut self, host: Host, cx: &mut Context<Self>) {
        if self.host.as_ref().is_some_and(|view| view.host == host) {
            return;
        }
        self.clear_active_repo(cx);
        self.dialog = None;
        self.context_menu.close();
        self.context_menu_target = None;
        // `scan_host` bumps `generation`, so a listing still in flight for
        // the closed repository is dropped, and with it the `apply_rows`
        // that would have cleared `loading`.
        self.loading = false;
        self.host = Some(HostView::new(host));
        self.scan_host(cx);
    }

    /// Scan the shown host: a quick pass for the listing, then a second one
    /// with `du` sizes, which can take tens of seconds on a real host.
    pub(super) fn scan_host(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.host.as_mut() else {
            return;
        };
        view.scanning = true;
        view.error = None;
        let host = view.host.clone();
        self.generation += 1;
        let generation = self.generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let quick_host = host.clone();
            let quick = cx
                .background_spawn(async move {
                    remote::scan(&quick_host, false).map_err(|e| e.to_string())
                })
                .await;
            let applied = this
                .update(cx, |this, cx| this.apply_scan(generation, quick, false, cx))
                .unwrap_or(false);
            if !applied {
                return;
            }
            let sized = cx
                .background_spawn(
                    async move { remote::scan(&host, true).map_err(|e| e.to_string()) },
                )
                .await;
            this.update(cx, |this, cx| this.apply_scan(generation, sized, true, cx))
                .ok();
        })
        .detach();
    }

    /// Apply a finished scan. Returns whether the listing was applied, so
    /// the caller knows whether the sized pass is still wanted.
    pub(super) fn apply_scan(
        &mut self,
        generation: u64,
        result: Result<Vec<RemoteRepo>, String>,
        sized: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        let Some(view) = self.host.as_mut() else {
            return false;
        };
        cx.notify();
        let mut repos = match result {
            Ok(repos) => repos,
            Err(e) => {
                view.error = Some(e);
                view.scanning = false;
                return false;
            }
        };
        if !sized {
            if let Some(previous) = &view.repos {
                carry_sizes(&mut repos, previous);
            }
        }
        remote::sort_by_size(&mut repos);
        view.selected.retain(|path| {
            repos
                .iter()
                .flat_map(|r| &r.worktrees)
                .any(|w| &w.info.path == path)
        });
        view.repos = Some(repos);
        view.sized = sized;
        view.scanning = !sized;
        true
    }

    pub(super) fn toggle_host_selection(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(view) = &mut self.host else {
            return;
        };
        if !view.selected.remove(&path) {
            view.selected.insert(path);
        }
        cx.notify();
    }

    // -------------------------------------------------------------
    // Sidebar menu
    // -------------------------------------------------------------

    fn open_host_context_menu(
        &mut self,
        name: String,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let items = vec![
            MenuItem::action("rescan", "Rescan").icon(icons::REFRESH),
            MenuItem::action("copy-destination", "Copy Destination").icon(icons::COPY),
            MenuItem::separator(),
            // Only the saved entry goes; nothing on the host is touched.
            MenuItem::action("forget", "Forget Host")
                .icon(icons::TRASH)
                .danger(),
        ];
        self.context_menu_target = Some(MenuTarget::Host(name));
        self.context_menu.open(position, items);
        cx.notify();
    }

    pub(super) fn handle_host_menu_action(&mut self, name: &str, id: &str, cx: &mut Context<Self>) {
        let Some(host) = self.hosts.iter().find(|h| h.name == name).cloned() else {
            return;
        };
        match id {
            "rescan" => {
                if self.host.as_ref().is_some_and(|view| view.host == host) {
                    self.scan_host(cx);
                } else {
                    self.select_host(host, cx);
                }
            }
            "copy-destination" => {
                let destination = host.destination;
                cx.spawn(async move |this, cx| {
                    let result = cx
                        .background_spawn(async move { data::copy_to_clipboard(&destination) })
                        .await;
                    this.update(cx, |this, cx| match result {
                        Ok(()) => this.set_info("destination copied", cx),
                        Err(e) => this.set_error(format!("copy failed: {e}"), cx),
                    })
                    .ok();
                })
                .detach();
            }
            "forget" => self.forget_host(name, cx),
            _ => {}
        }
    }

    fn forget_host(&mut self, name: &str, cx: &mut Context<Self>) {
        match remote::forget_host(name) {
            Ok(_) => {
                self.hosts = remote::load_hosts();
                if self
                    .host
                    .as_ref()
                    .is_some_and(|view| view.host.name == name)
                {
                    self.host = None;
                }
                self.set_info(format!("forgot host {name}"), cx);
            }
            Err(e) => self.set_error(format!("could not save the host list: {e}"), cx),
        }
    }

    // -------------------------------------------------------------
    // Add Host
    // -------------------------------------------------------------

    pub(super) fn open_add_host_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay_open() {
            return;
        }
        let state = AddHostState::new(window, cx);
        let name_focus = state.name.focus_handle(cx);
        self.host_dialog = Some(HostDialog::Add(state));
        window.focus(&name_focus);
        cx.notify();
    }

    pub(super) fn submit_add_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(HostDialog::Add(state)) = &mut self.host_dialog else {
            return;
        };
        let name = state.name.read(cx).value().to_string();
        let destination = state.destination.read(cx).value().to_string();
        let roots = state
            .roots
            .read(cx)
            .value()
            .split(',')
            .map(str::to_string)
            .collect();
        let host = match Host::new(&name, &destination, roots) {
            Ok(host) => host,
            Err(e) => {
                state.error = Some(e.to_string());
                cx.notify();
                return;
            }
        };
        if let Err(e) = remote::upsert_host(host.clone()) {
            state.error = Some(format!("could not save the host list: {e}"));
            cx.notify();
            return;
        }
        self.hosts = remote::load_hosts();
        self.close_dialog(window, cx);
        self.select_host(host, cx);
    }

    // -------------------------------------------------------------
    // Remove and Clean Up
    // -------------------------------------------------------------

    /// Remove Selected (⌘⌫) while a host is shown. Never deletes branches.
    pub(super) fn open_host_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay_open() {
            return;
        }
        let Some(view) = &self.host else {
            return;
        };
        if view.busy || view.selected.is_empty() {
            return;
        }
        let protected = protected_branches();
        let groups: Vec<RemovalGroup> = view
            .repos
            .iter()
            .flatten()
            .filter_map(|repo| {
                let picked: Vec<WorktreeInfo> = repo
                    .worktrees
                    .iter()
                    .filter(|w| view.selected.contains(&w.info.path))
                    .map(|w| w.info.clone())
                    .collect();
                let candidates = with_sizes(repo, prune::selection_candidates(picked, &protected));
                (!candidates.is_empty()).then(|| (repo.path.clone(), candidates))
            })
            .collect();
        if groups.is_empty() {
            self.set_error(
                "nothing to remove — the selection is only protected branches",
                cx,
            );
            return;
        }
        self.open_host_confirm(ConfirmKind::Remove, groups, window, cx);
    }

    /// A repository's Clean Up button: the same selection as `wtm prune
    /// --merged --gone --detached`, deleting merged and gone branches.
    pub(super) fn open_host_cleanup(
        &mut self,
        repo_path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_open() {
            return;
        }
        let Some(view) = &self.host else {
            return;
        };
        if view.busy {
            return;
        }
        let Some(repo) = view.repos.iter().flatten().find(|r| r.path == repo_path) else {
            return;
        };
        let candidates = with_sizes(
            repo,
            remote::prune_candidates(repo, &protected_branches(), true, true, true),
        );
        if candidates.is_empty() {
            let message = format!("nothing to clean up in {}", repo.name);
            self.set_info(message, cx);
            return;
        }
        let groups = vec![(repo.path.clone(), candidates)];
        self.open_host_confirm(ConfirmKind::CleanUp, groups, window, cx);
    }

    fn open_host_confirm(
        &mut self,
        kind: ConfirmKind,
        groups: Vec<RemovalGroup>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.host_dialog = Some(HostDialog::Confirm(HostConfirmState {
            kind,
            groups,
            force: false,
        }));
        window.focus(&self.dialog_safe_focus);
        cx.notify();
    }

    fn toggle_host_force(&mut self, cx: &mut Context<Self>) {
        if let Some(HostDialog::Confirm(state)) = &mut self.host_dialog {
            state.force = !state.force;
        }
        cx.notify();
    }

    pub(super) fn confirm_host_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(HostDialog::Confirm(state)) = &self.host_dialog else {
            return;
        };
        let Some(view) = self.host.as_mut() else {
            return;
        };
        if view.busy {
            return;
        }
        view.busy = true;
        let host = view.host.clone();
        let groups = state.groups.clone();
        let force = state.force;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let host_name = host.name.clone();
            let report = cx
                .background_spawn(async move { remove_groups(&host, &groups, force) })
                .await;
            this.update(cx, |this, cx| {
                this.finish_host_remove(&host_name, report, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Always report; only touch the pane and its confirmation if the host
    /// that ran the removal is still the one shown.
    fn finish_host_remove(
        &mut self,
        host_name: &str,
        report: HostRemoveReport,
        cx: &mut Context<Self>,
    ) {
        let still_shown = self
            .host
            .as_ref()
            .is_some_and(|view| view.host.name == host_name);
        if still_shown {
            if matches!(self.host_dialog, Some(HostDialog::Confirm(_))) {
                self.host_dialog = None;
            }
            if let Some(view) = &mut self.host {
                view.busy = false;
            }
        }

        let mut parts = vec![format!(
            "removed {} worktree{} on {host_name}",
            report.removed,
            plural(report.removed)
        )];
        if report.freed > 0 {
            parts.push(format!("freed ~{}", remote::format_size(report.freed)));
        }
        if report.failures.is_empty() {
            self.set_info(parts.join(" · "), cx);
        } else {
            parts.push(format!("failed: {}", report.failures.join("; ")));
            self.set_error(parts.join(" · "), cx);
        }
        if still_shown {
            self.scan_host(cx);
        }
    }

    // -------------------------------------------------------------
    // Rendering: sidebar
    // -------------------------------------------------------------

    pub(super) fn render_host_section(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let shown = self.host.as_ref().map(|view| view.host.name.as_str());
        div()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_2))
            .pt(px(theme::SPACE_12))
            .child(
                div()
                    .px(px(theme::SPACE_8))
                    .pb(px(theme::SPACE_2))
                    .text_size(px(ui::TEXT_XS))
                    .text_color(theme.text_ghost)
                    .child("Hosts"),
            )
            .children(self.hosts.iter().map(|host| {
                self.render_host_row(host, shown == Some(host.name.as_str()), theme, cx)
            }))
            .child(
                ui::action_row("add-host", icons::PLUS, "Add Host…", None, theme).on_click(
                    cx.listener(|this, _, window, cx| this.open_add_host_dialog(window, cx)),
                ),
            )
    }

    fn render_host_row(
        &self,
        host: &Host,
        is_shown: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        ui::row(
            SharedString::from(format!("host-{}", host.name)),
            is_shown,
            theme,
        )
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_4))
        .child(
            text_box(truncate_tail(&host.name, SIDEBAR_PATH_MAX_CHARS))
                .line_height(px(18.0))
                .text_size(px(ui::TEXT_BASE))
                .text_color(theme.text),
        )
        .child(
            text_box(truncate_tail(&host.destination, SIDEBAR_PATH_MAX_CHARS))
                .font_family(theme.font_mono)
                .text_size(px(ui::TEXT_SM))
                .text_color(theme.text_muted),
        )
        .on_click(cx.listener({
            let host = host.clone();
            move |this, _, _window, cx| this.select_host(host.clone(), cx)
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener({
                let name = host.name.clone();
                move |this, event: &MouseDownEvent, _window, cx| {
                    this.open_host_context_menu(name.clone(), event.position, cx);
                }
            }),
        )
    }

    // -------------------------------------------------------------
    // Rendering: host pane
    // -------------------------------------------------------------

    pub(super) fn render_host_pane(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = &self.host else {
            return div().into_any_element();
        };
        let theme = self.chrome_theme(cx);

        let summary = match &view.repos {
            None if view.scanning => "scanning…".to_string(),
            None if view.error.is_some() => "scan failed".to_string(),
            None => "not scanned".to_string(),
            Some(repos) => {
                let worktrees: usize = repos.iter().map(|r| r.worktrees.len()).sum();
                let mut text = format!(
                    "{} repositor{} · {worktrees} worktree{}",
                    repos.len(),
                    if repos.len() == 1 { "y" } else { "ies" },
                    plural(worktrees)
                );
                if view.sized {
                    let total: u64 = repos.iter().map(RemoteRepo::size_bytes).sum();
                    text.push_str(&format!(" · {}", remote::format_size(total)));
                } else if view.scanning {
                    text.push_str(" · sizing…");
                }
                text
            }
        };

        let header = div()
            .flex()
            .flex_wrap()
            .min_w_0()
            .items_center()
            .justify_between()
            .gap(px(theme::SPACE_16))
            .px(px(theme::SPACE_16))
            .pb(px(theme::SPACE_8))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .min_w_0()
                    .items_center()
                    .gap(px(theme::SPACE_8))
                    .text_size(px(ui::TEXT_SM))
                    .child(
                        text_box(truncate_tail(&view.host.destination, 40))
                            .font_family(theme.font_mono)
                            .text_color(theme.text_muted),
                    )
                    .child(text_box(summary).text_color(theme.text_faint)),
            )
            .child(
                ui::toolbar_button(
                    "host-rescan",
                    icons::REFRESH,
                    if view.scanning {
                        "Scanning…"
                    } else {
                        "Rescan"
                    },
                    ButtonVariant::Secondary,
                    &theme,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.scan_host(cx))),
            );

        let body: AnyElement = match &view.repos {
            None if view.scanning => ui::empty_hint(
                format!("Scanning {}…", truncate_tail(&view.host.destination, 40)),
                &theme,
            )
            .into_any_element(),
            None => {
                ui::empty_hint("Nothing to show until a scan succeeds.", &theme).into_any_element()
            }
            Some(repos) if repos.is_empty() => {
                let roots = if view.host.roots.is_empty() {
                    "~".to_string()
                } else {
                    view.host.roots.join(", ")
                };
                ui::empty_state(
                    icons::FOLDER_OPEN,
                    "No repositories found",
                    truncate_tail(
                        &format!(
                            "Nothing under {roots}. To look elsewhere, add the host again \
                             with the same name and other roots."
                        ),
                        160,
                    ),
                    None,
                    &theme,
                )
                .into_any_element()
            }
            Some(repos) => self.render_host_repos(view, repos, window, &theme, cx),
        };

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
                    .child(header)
                    .when_some(view.error.clone(), |this, error| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(theme::SPACE_12))
                                .px(px(theme::SPACE_16))
                                .pb(px(theme::SPACE_8))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(ui::inline_error(error, &theme)),
                                )
                                .child(
                                    ui::button(
                                        "host-retry",
                                        "Retry",
                                        ButtonVariant::Secondary,
                                        &theme,
                                    )
                                    .on_click(
                                        cx.listener(|this, _, _window, cx| this.scan_host(cx)),
                                    ),
                                ),
                        )
                    })
                    .when(!view.selected.is_empty(), |this| {
                        this.child(self.render_selection_bar(
                            view.selected.len(),
                            "click toggles · ⎋ clears",
                            &theme,
                            cx,
                        ))
                    })
                    .child(body),
            )
            .into_any_element()
    }

    fn render_host_repos(
        &self,
        view: &HostView,
        repos: &[RemoteRepo],
        window: &Window,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let card_width = self.worktree_row_card_width(window);
        let mut children: Vec<AnyElement> = Vec::new();
        let mut row_ix = 0;
        for (repo_ix, repo) in repos.iter().enumerate() {
            children.push(Self::render_host_repo_header(
                repo_ix, repo, card_width, theme, cx,
            ));
            for worktree in &repo.worktrees {
                let selected = view.selected.contains(&worktree.info.path);
                let gutter = div()
                    .flex_none()
                    .w(px(ROW_CHECKBOX_SIZE))
                    .h(px(ROW_CHECKBOX_SIZE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(selected, |this| {
                        this.child(ui::icon(icons::CHECK, 10.0, theme.accent))
                    });
                let row = worktree_list::render_row(
                    &worktree.info,
                    row_ix,
                    selected,
                    false,
                    worktree.size_bytes.map(remote::format_size),
                    card_width,
                    theme,
                    cx,
                )
                .flex_1()
                .min_w_0();
                let row = if worktree.info.is_main {
                    row
                } else {
                    let path = worktree.info.path.clone();
                    row.on_click(cx.listener(move |this, _, _window, cx| {
                        this.toggle_host_selection(path.clone(), cx);
                    }))
                };
                children.push(
                    div()
                        .px(px(theme::SPACE_8))
                        .pb(px(theme::LIST_ROW_PITCH - theme::LIST_ROW_HEIGHT))
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE_6))
                        .child(gutter)
                        .child(row)
                        .into_any_element(),
                );
                row_ix += 1;
            }
        }

        let edges = ui::scroll_edges(
            f32::from(view.scroll.offset().y),
            f32::from(view.scroll.max_offset().height),
        );
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("host-scroll")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&view.scroll)
                    .px(px(theme::SPACE_8))
                    .pb(px(theme::SPACE_8))
                    .children(children),
            )
            .when(edges.leading, |this| {
                this.child(ui::scroll_fade_top(theme.bg, theme::SPACE_24))
            })
            .when(edges.trailing, |this| {
                this.child(ui::scroll_fade_bottom(theme.bg, theme::SPACE_24))
            })
            .child(ui::scrollbar("host-scrollbar", &view.scroll))
            .into_any_element()
    }

    /// A repository's heading, aligned with the worktree cards under it:
    /// the same checkbox gutter, then the card's own inner padding.
    fn render_host_repo_header(
        repo_ix: usize,
        repo: &RemoteRepo,
        card_width: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = repo.worktrees.len();
        let mut stats = format!("{count} worktree{}", plural(count));
        if repo.worktrees.iter().any(|w| w.size_bytes.is_some()) {
            stats.push_str(&format!(" · {}", remote::format_size(repo.size_bytes())));
        }
        let inner = card_width - theme::SPACE_8 * 2.0;
        let reserved = stats.chars().count() as f32 * ui::CHAR_WIDTH_APPROX
            + CLEANUP_BUTTON_WIDTH
            + theme::SPACE_8 * 4.0;
        let budget = ((inner - reserved) / ui::CHAR_WIDTH_APPROX).max(0.0) as usize;
        let name = truncate_tail(&repo.name, budget.clamp(6, 32));
        let path = truncate_path_tail(
            &ui::display_path(&repo.path),
            budget.saturating_sub(name.chars().count()).max(8),
        );
        let repo_path = repo.path.clone();

        div()
            .px(px(theme::SPACE_8))
            .pt(px(theme::SPACE_12))
            .pb(px(theme::SPACE_4))
            .flex()
            .items_center()
            .gap(px(theme::SPACE_6))
            .child(div().flex_none().w(px(ROW_CHECKBOX_SIZE)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(theme::SPACE_8))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE_8))
                    .overflow_hidden()
                    .child(
                        text_box(name)
                            .text_size(px(ui::TEXT_BASE))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text),
                    )
                    .child(
                        text_box(path)
                            .font_family(theme.font_mono)
                            .text_size(px(ui::TEXT_SM))
                            .text_color(theme.text_muted),
                    )
                    .child(div().flex_1())
                    .child(
                        text_box(stats)
                            .text_size(px(ui::TEXT_SM))
                            .text_color(theme.text_faint),
                    )
                    // A main worktree alone has nothing Clean Up could remove.
                    .when(count > 1, |this| {
                        this.child(
                            ui::button(
                                ("host-cleanup", repo_ix),
                                "Clean Up",
                                ButtonVariant::Secondary,
                                theme,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.open_host_cleanup(&repo_path, window, cx);
                                },
                            )),
                        )
                    }),
            )
            .into_any_element()
    }

    // -------------------------------------------------------------
    // Rendering: modals
    // -------------------------------------------------------------

    pub(super) fn render_host_dialog(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        match &self.host_dialog {
            Some(HostDialog::Add(state)) => self.render_add_host_dialog(state, theme, cx),
            Some(HostDialog::Confirm(state)) => self.render_host_confirm_dialog(state, theme, cx),
            None => div().into_any_element(),
        }
    }

    fn render_add_host_dialog(
        &self,
        state: &AddHostState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let destination = state.destination.read(cx).value().trim();
        let destination = if destination.is_empty() {
            "<destination>".to_string()
        } else {
            truncate_tail(destination, 40)
        };

        let body = div()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_12))
            .px(px(theme::SPACE_16))
            .py(px(theme::SPACE_12))
            .child(labeled_field("Name", state.name.clone(), theme))
            .child(labeled_field(
                "SSH destination",
                state.destination.clone(),
                theme,
            ))
            .child(labeled_field(
                "Roots (comma-separated, optional)",
                state.roots.clone(),
                theme,
            ))
            .child(
                div()
                    .text_size(px(ui::TEXT_XS))
                    .text_color(theme.text_ghost)
                    .child(format!(
                        "Key-based login only: wtm never asks for a password. \
                         Run “ssh {destination}” in a terminal once first."
                    )),
            )
            .when_some(state.error.clone(), |this, error| {
                this.child(ui::inline_error(error, theme))
            })
            .child(
                ui::modal_footer(theme)
                    .child(
                        ui::button("add-host-cancel", "Cancel", ButtonVariant::Secondary, theme)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_dialog(window, cx)),
                            ),
                    )
                    .child(
                        ui::button(
                            "add-host-confirm",
                            "Add Host",
                            ButtonVariant::Primary,
                            theme,
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.submit_add_host(window, cx)),
                        ),
                    ),
            );

        let card = ui::modal_card(440.0, theme)
            .id("add-host-dialog-card")
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(ui::modal_header(
                "Add Host",
                Some("List and clean up repositories on another machine over SSH"),
                theme,
            ))
            .child(body);
        present_modal("add-host-dialog", card, cx)
    }

    fn render_host_confirm_dialog(
        &self,
        state: &HostConfirmState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.host.as_ref().is_some_and(|view| view.busy);
        let host_name = self
            .host
            .as_ref()
            .map(|view| view.host.name.as_str())
            .unwrap_or_default();
        let candidates = || state.groups.iter().flat_map(|(_, entries)| entries);
        let count = candidates().count();
        let has_dirty = candidates()
            .any(|(c, _)| !c.info.is_missing && c.info.status.as_ref().is_some_and(|s| s.dirty));
        let freed: u64 = candidates().filter_map(|(_, size)| *size).sum();

        let (title, subtitle, confirm_label) = match state.kind {
            ConfirmKind::Remove => ("Remove Worktrees", format!("on {host_name}"), "Remove"),
            ConfirmKind::CleanUp => {
                let repo = state
                    .groups
                    .first()
                    .and_then(|(path, _)| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (
                    "Clean Up",
                    format!(
                        "Merged, upstream-gone, and detached worktrees in {repo} on {host_name}"
                    ),
                    "Clean Up",
                )
            }
        };

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_12))
            .px(px(theme::SPACE_16))
            .py(px(theme::SPACE_12))
            .child(
                div()
                    .text_size(px(ui::TEXT_BASE))
                    .text_color(theme.text_muted)
                    .child(format!(
                        "{count} worktree{} will be removed:",
                        plural(count)
                    )),
            );

        if has_dirty {
            body = body
                .child(uncommitted_changes_warning(
                    "Some have uncommitted changes. Without Force, git keeps those.",
                    theme,
                ))
                .child(
                    dialogs::render_toggle(
                        "host-confirm-force",
                        "Force (discard uncommitted changes)",
                        state.force,
                        false,
                        theme,
                    )
                    .on_click(cx.listener(|this, _, _window, cx| this.toggle_host_force(cx))),
                );
        }

        body = body.child(
            div()
                .id("host-confirm-list")
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_4))
                .max_h(px(260.0))
                .overflow_y_scroll()
                .children(state.groups.iter().flat_map(move |(repo_path, entries)| {
                    std::iter::once(
                        div()
                            .pt(px(theme::SPACE_4))
                            .font_family(theme.font_mono)
                            .text_size(px(ui::TEXT_XS))
                            .text_color(theme.text_ghost)
                            .child(truncate_path_tail(&ui::display_path(repo_path), 56))
                            .into_any_element(),
                    )
                    .chain(entries.iter().map(move |(c, _)| {
                        dialogs::render_candidate_row(c, theme).into_any_element()
                    }))
                })),
        );

        if freed > 0 {
            body = body.child(
                div()
                    .text_size(px(ui::TEXT_SM))
                    .text_color(theme.text_muted)
                    .child(format!("Frees about {}.", remote::format_size(freed))),
            );
        }
        body = body.child(destructive_count_line(count, "remove", theme));
        if busy {
            body = body.child(
                div()
                    .text_size(px(ui::TEXT_SM))
                    .text_color(theme.text_muted)
                    .child(format!("Removing on {host_name}…")),
            );
        }

        let confirm = ui::button(
            "host-confirm-confirm",
            confirm_label,
            ButtonVariant::Danger,
            theme,
        );
        let confirm = if busy {
            ui::disabled(confirm.opacity(0.4)).into_any_element()
        } else {
            confirm
                .on_click(cx.listener(|this, _, _window, cx| this.confirm_host_dialog(cx)))
                .into_any_element()
        };
        body = body.child(
            ui::modal_footer(theme)
                .child(
                    // Focus lands here on open: the safe action.
                    ui::button(
                        "host-confirm-cancel",
                        "Cancel",
                        ButtonVariant::Secondary,
                        theme,
                    )
                    .track_focus(&self.dialog_safe_focus)
                    .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
                )
                .child(confirm),
        );

        let card = ui::modal_card(440.0, theme)
            .id("host-confirm-dialog-card")
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(ui::modal_header(title, Some(&subtitle), theme))
            .child(body);
        present_modal("host-confirm-dialog", card, cx)
    }
}

#[cfg(test)]
mod tests {
    use wtm::model::WorktreeStatus;
    use wtm::remote::RemoteWorktree;

    use super::*;

    fn worktree(path: &str, is_main: bool, size_bytes: Option<u64>) -> RemoteWorktree {
        RemoteWorktree {
            info: WorktreeInfo {
                name: path.to_string(),
                path: PathBuf::from(path),
                branch: None,
                head: None,
                is_main,
                is_missing: false,
                is_locked: false,
                lock_reason: None,
                head_time: None,
                is_prunable: false,
                status: Some(WorktreeStatus {
                    dirty: false,
                    dirty_count: 0,
                    ahead: None,
                    behind: None,
                    upstream_gone: false,
                    merged: false,
                }),
            },
            size_bytes,
        }
    }

    fn repo(path: &str, worktrees: Vec<RemoteWorktree>) -> RemoteRepo {
        RemoteRepo {
            name: path.to_string(),
            path: PathBuf::from(path),
            worktrees,
        }
    }

    #[test]
    fn carry_sizes_fills_only_unknown_sizes_of_known_paths() {
        let previous = vec![repo(
            "/a",
            vec![
                worktree("/a", true, Some(7)),
                worktree("/a-x", false, Some(3)),
            ],
        )];
        let mut repos = vec![repo(
            "/a",
            vec![
                worktree("/a", true, None),
                worktree("/a-x", false, Some(9)),
                worktree("/a-new", false, None),
            ],
        )];

        carry_sizes(&mut repos, &previous);

        let sizes: Vec<Option<u64>> = repos[0].worktrees.iter().map(|w| w.size_bytes).collect();
        assert_eq!(sizes, [Some(7), Some(9), None]);
    }
}

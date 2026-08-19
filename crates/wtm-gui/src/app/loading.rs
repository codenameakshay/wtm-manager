//! Repository activation and data loading: opening/switching repositories,
//! the two-pass (fast, then with-status) worktree reload, the filesystem
//! watcher that keeps `rows` in sync with changes made outside the app, and
//! loading the detail panel's data for whatever row is selected.
//!
//! This module owns the `generation`/`details_generation` staleness guards
//! and is the only place that calls `data::list_worktrees` and
//! `data::worktree_details`. It does not own selection (see `selection`) or
//! any dialog's business logic (see `dialog_actions`) — it only produces
//! the data those act on.

use super::*;

impl WtmApp {
    /// Show a repository: record it in the registry, then load its worktrees
    /// entirely in the background. Used for every activation *after* the
    /// first — the sidebar's repo switcher — where the window is already on
    /// screen and gpui will paint whenever a listing lands, so there is no
    /// reason to block on anything here.
    pub(super) fn activate_repo(&mut self, repo: OpenRepo, cx: &mut Context<Self>) {
        self.begin_activate_repo(repo, cx);
        self.reload(cx);
    }

    /// The bookkeeping every activation needs regardless of how its listing
    /// gets loaded: record the repo in the registry, drop state that
    /// referenced the previous one, and reset the row set. Split out of
    /// `activate_repo` so `new`'s startup path can run this and then seed
    /// `rows` synchronously (`seed_initial_rows`) instead of going through
    /// `activate_repo`'s all-async `reload`.
    pub(super) fn begin_activate_repo(&mut self, repo: OpenRepo, cx: &mut Context<Self>) {
        // A dialog holds state (a snapshot worktree, a picker built from the
        // old repo's branches) that stops making sense the moment the
        // active repository changes out from under it. A right-click menu
        // targeting a row of the old repo is in the same position.
        self.dialog = None;
        self.context_menu.close();
        self.context_menu_target = None;
        if let Err(e) = registry::remember(repo.path(), repo.name()) {
            // A registry that cannot be written is a papercut, not a failure:
            // the session still works, so say so and carry on.
            self.set_status(format!("could not save the repo list: {e}"), true);
        }
        self.repos = registry::load().entries();
        self.prefs.last_repo = Some(repo.path().to_path_buf());
        self.save_prefs();
        self.active = Some(repo);
        self.rows.clear();
        self.selected = None;
        // Worktree paths from the old repository are meaningless under the
        // new one (and could, in principle, collide with a path the new
        // repository also happens to use) — drop the file browser's
        // per-worktree cache outright rather than leaving it to be
        // reclaimed piecemeal by `load_panel_data`.
        self.file_trees.clear();
        // Clears any detail data left over from the previous repository —
        // `load_details_for_selection` sees `self.selected` is now `None`
        // and degrades to "nothing loaded" rather than showing a stale
        // worktree's commits under the new repo's name for a frame. It also
        // clears the Files/Changes tabs' data the same way — see
        // `load_panel_data`.
        self.load_details_for_selection(cx);
    }

    /// Startup only, called once from `new`: try the fast (no-status)
    /// listing *synchronously*, on the UI thread, before `new` returns and
    /// this view gets painted for the first time.
    ///
    /// This exists because of a real gpui/macOS constraint: the platform
    /// backend only (re)starts the `CVDisplayLink` that drives repaints when
    /// Cocoa reports a window occlusion/visibility change, which happens on
    /// first becoming key/visible or on a screen change — not on `cx.notify`
    /// or `window.refresh`, which only mark state dirty. If this window
    /// never becomes key (it opens behind another app that keeps focus, for
    /// instance), the *only* frame ever shown is the one painted
    /// synchronously while the window is being set up. Before this change
    /// that frame showed `rows` empty and `loading` true — "0 worktrees ·
    /// loading status…" — because the fast listing only ever ran on a
    /// background task kicked off after the window (and that first frame)
    /// already existed. Doing the fast pass here instead means that first
    /// frame has real rows in it.
    ///
    /// `list_worktrees(_, false)` is the deliberately cheap path (see its
    /// doc comment: it skips the dirty/ahead/behind/merged walk entirely and
    /// just enumerates git's worktree registry), which is what makes running
    /// it synchronously here bounded — it costs whatever `wtm list` already
    /// costs interactively, not a full status walk. The expensive
    /// with-status pass stays exactly where it was: kicked off in the
    /// background via `reload_status_pass` below, so status pills still
    /// fill in asynchronously once it lands.
    ///
    /// A broken repository must never block the window on the UI thread or
    /// panic, so a failed synchronous attempt is silently discarded in favor
    /// of `reload`'s ordinary all-async two-pass load — the same path a
    /// repo switch always takes, and the same "loading…" first frame this
    /// app showed everywhere before this change.
    pub(super) fn seed_initial_rows(&mut self, repo: OpenRepo, cx: &mut Context<Self>) {
        match data::list_worktrees(&repo, false) {
            Ok(rows) => {
                // `self.generation` is still its freshly constructed value
                // (0) here — nothing has bumped it yet — so this is simply
                // the same `apply_rows` call the async fast pass would make,
                // just performed before the first paint instead of after it.
                let generation = self.generation;
                self.apply_rows(generation, Ok(rows), false, cx);
                self.reload_status_pass(cx);
            }
            Err(_) => self.reload(cx),
        }
    }

    /// Load the active repository's worktrees in two passes: a fast listing
    /// without status so the list paints immediately, then the full listing
    /// with dirty/ahead/behind/merged computed in parallel.
    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        self.reload_impl(true, cx);
    }

    /// The with-status pass only, skipping the fast pass `reload` normally
    /// starts with. Used exactly once, right after `seed_initial_rows` has
    /// already run the fast pass synchronously and seeded `rows` from it —
    /// repeating it here would just be redundant work for a result the
    /// screen already reflects.
    fn reload_status_pass(&mut self, cx: &mut Context<Self>) {
        self.reload_impl(false, cx);
    }

    fn reload_impl(&mut self, include_fast_pass: bool, cx: &mut Context<Self>) {
        let Some(repo) = self.active.clone() else {
            return;
        };

        self.generation += 1;
        self.loading = true;
        self.awaiting_status = true;
        let generation = self.generation;

        let fast_repo = repo.clone();
        cx.spawn(async move |this, cx| {
            if include_fast_pass {
                let fast = cx
                    .background_spawn(async move { data::list_worktrees(&fast_repo, false) })
                    .await;
                this.update(cx, |this, cx| this.apply_rows(generation, fast, false, cx))
                    .ok();
            }

            let full = cx
                .background_spawn(async move { data::list_worktrees(&repo, true) })
                .await;
            this.update(cx, |this, cx| this.apply_rows(generation, full, true, cx))
                .ok();
        })
        .detach();
    }

    /// Apply a finished listing, ignoring results from a superseded load.
    fn apply_rows(
        &mut self,
        generation: u64,
        result: Result<Vec<WorktreeInfo>, String>,
        with_status: bool,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        if with_status {
            self.loading = false;
            self.awaiting_status = false;
        }

        match result {
            Ok(rows) => {
                // Capture the current selection by *worktree identity*
                // (path), not by index, before `self.rows` is replaced —
                // this reload (a manual ⌘R, the fast/with-status pass pair
                // every reload runs, or a background `RepoWatcher` tick the
                // user never asked for) can reorder rows out from under an
                // unchanged selection the moment a status change moves one
                // under `SortMode::Status`/`Recent`, or resize the set
                // entirely if a worktree was added/removed outside the app.
                // The old rule — "keep index `ix` if it's still in range" —
                // silently re-points `selected` at whatever row now happens
                // to occupy that slot, which reads to the user as the
                // selection randomly jumping to an unrelated worktree after
                // an idle moment. `resort_preserving_selection` already
                // solved this same problem for a sort-mode change by
                // looking the selection back up by path; this mirrors it.
                // Switching to a *different* repository is unaffected:
                // `begin_activate_repo` always clears `self.rows`/
                // `self.selected` first, so `anchor_path` is `None` here
                // and the legitimate "no previous selection → start at the
                // top" branch below still runs.
                let anchor_path = self
                    .selected
                    .and_then(|ix| self.rows.get(ix))
                    .map(|row| row.path.clone());
                let multi_paths: Vec<PathBuf> = self
                    .multi_selected
                    .iter()
                    .filter_map(|&ix| self.rows.get(ix).map(|row| row.path.clone()))
                    .collect();

                self.rows = rows;
                // Every listing is shown in the currently active sort mode,
                // not whatever order the backend happened to return — this
                // must run before the selection logic below, which resolves
                // indices against the final row order.
                worktree_list::sort_rows(&mut self.rows, self.sort_mode, &self.activity);
                // A pending selection (set right after a create) wins over
                // both the identity lookup and the index-clamp fallback —
                // but only once: `take()` consumes it so a later manual
                // reload falls back to the normal behavior.
                let pending = self
                    .pending_select
                    .take()
                    .and_then(|branch| self.rows.iter().position(|r| r.display_name() == branch));
                self.selected = pending.or_else(|| {
                    anchor_path
                        .and_then(|path| self.rows.iter().position(|r| r.path == path))
                        .or_else(|| match self.selected {
                            // The previously selected worktree is genuinely
                            // gone — removed or pruned outside the app, or
                            // by this very reload — so there is no identity
                            // left to look up. Falling back to the clamped
                            // index (not the top) is deliberate: it keeps
                            // "remove the selected worktree" landing on
                            // whichever row took its place, the same
                            // behavior this app has always had for that
                            // case.
                            _ if self.rows.is_empty() => None,
                            Some(ix) if ix < self.rows.len() => Some(ix),
                            Some(_) => Some(self.rows.len() - 1),
                            None => Some(0),
                        })
                });
                self.multi_selected = multi_paths
                    .iter()
                    .filter_map(|path| self.rows.iter().position(|r| &r.path == path))
                    .collect();
                // Rows just changed wholesale (a reload) — a filter that
                // matched some of the old set may match a different subset
                // of the new one, and the pending-selection branch above
                // can itself have picked a now-hidden row; both are why
                // this runs *after* it rather than folding into it.
                self.clamp_selection_to_filter(cx);
                self.sync_watcher(cx);
                self.load_details_for_selection(cx);
                self.spawn_activity_load(generation, cx);
                // A right-click menu open for a worktree row that a
                // background refresh just removed (the worktree was deleted
                // or pruned outside the app) would otherwise keep offering
                // actions — Open in Editor, Remove — for a path that no
                // longer exists.
                if let Some(MenuTarget::Worktree(path)) = &self.context_menu_target {
                    if !self.rows.iter().any(|row| &row.path == path) {
                        self.context_menu.close();
                        self.context_menu_target = None;
                    }
                }
                if with_status {
                    // A background refresh must never silently erase an
                    // error the user hasn't read yet — only a status that
                    // was purely informational gets cleared here.
                    if self.status.as_ref().is_some_and(|s| !s.error) {
                        self.status = None;
                    }
                }
            }
            Err(e) => {
                self.loading = false;
                self.set_status(format!("could not list worktrees: {e}"), true);
            }
        }
        cx.notify();
    }

    // -------------------------------------------------------------
    // Worktree activity (staleness / Recent-mode sorting)
    // -------------------------------------------------------------

    /// Kick off `data::worktree_activity` for the rows just applied, in the
    /// background — this is exactly the kind of git2-touching call the
    /// module doc forbids on the UI thread. Guarded by `generation`, the
    /// same counter `apply_rows` itself was just called with, so a slow
    /// activity load for a repository or listing the user has since
    /// navigated away from can never land on a newer one — see
    /// `apply_activity`.
    fn spawn_activity_load(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            // Nothing to look up. Leaving a stale `activity` map around is
            // harmless (the next repo's rows won't match its paths and
            // `begin_activate_repo` clearing it is not this method's job),
            // but there is also nothing useful to spawn a task for.
            return;
        }
        let paths: Vec<PathBuf> = self.rows.iter().map(|row| row.path.clone()).collect();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { data::worktree_activity(&paths) })
                .await;
            this.update(cx, |this, cx| this.apply_activity(generation, result, cx))
                .ok();
        })
        .detach();
    }

    /// Apply a finished `worktree_activity` load, ignoring one superseded by
    /// a newer listing — same `generation`-guard shape as `apply_rows`
    /// itself. Age only ever affects display (a row's meta line) and
    /// `Recent`-mode ordering, so landing this re-sorts and re-translates
    /// the selection (`resort_preserving_selection`, in `selection.rs`)
    /// rather than re-running the whole `apply_rows` pipeline.
    fn apply_activity(
        &mut self,
        generation: u64,
        result: HashMap<PathBuf, i64>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        self.activity = result;
        self.resort_preserving_selection(cx);
    }

    // -------------------------------------------------------------
    // Live refresh (filesystem watcher)
    // -------------------------------------------------------------

    /// (Re)target the watcher at the active repository's git directory and
    /// its current worktree paths. A no-op when both already match what is
    /// being watched, which is what keeps this from tearing down and
    /// recreating OS watch descriptors on every reload — including a reload
    /// the watcher itself triggered, whose own `apply_rows` call reaches
    /// here with an unchanged path set.
    ///
    /// This, together with `on_watcher_change`'s in-flight check, is the
    /// whole loop-prevention story: retargeting touches no files (it only
    /// opens watch descriptors), and nothing on the read path this app
    /// takes to list worktrees or compute status ever writes into `.git` —
    /// so a watcher-triggered reload can never itself produce a filesystem
    /// event for the watcher to react to.
    fn sync_watcher(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.active.as_ref() else {
            self.watcher = None;
            self.watched = None;
            return;
        };

        // See `crate::watcher::DISABLED_FOR_TESTS`'s doc comment: starting a
        // real watcher inside a `#[gpui::test]` hangs `run_until_parked`
        // forever, so `app::integration_tests` flips this once per test and
        // every flow keeps working exactly as it would with a watcher that
        // simply failed to start (manual reload still works — see this
        // method's callers).
        #[cfg(test)]
        if crate::watcher::DISABLED_FOR_TESTS.load(std::sync::atomic::Ordering::Relaxed) {
            self.watcher = None;
            self.watched = None;
            return;
        }

        let git_dir = repo.ctx.git_dir.clone();
        let mut worktrees: Vec<PathBuf> = self.rows.iter().map(|row| row.path.clone()).collect();
        worktrees.sort();
        let target = (git_dir.clone(), worktrees.clone());
        if self.watched.as_ref() == Some(&target) {
            return;
        }

        let started = match &mut self.watcher {
            Some(watcher) => watcher.watch(git_dir, worktrees, cx, Self::on_watcher_change),
            None => {
                self.watcher = RepoWatcher::new(git_dir, worktrees, cx, Self::on_watcher_change);
                self.watcher.is_some()
            }
        };
        // On failure, leave `watched` as `None` rather than recording a
        // target that isn't actually being watched — see `RepoWatcher`'s own
        // docs on degrading to manual-refresh-only rather than pointing at
        // stale paths. Never surfaced as a status message: a watcher that
        // can't start must be invisible, not a papercut the user has to read
        // about on every repo switch.
        self.watched = started.then_some(target);
    }

    /// Called on the gpui foreground when the watcher notices a change.
    /// Reuses the exact same `reload` path ⌘R uses, but skips triggering a
    /// second one while a reload is already in flight — a burst of git
    /// operations (a rebase, `git worktree add` followed immediately by a
    /// commit) can debounce into more than one notification, and stacking a
    /// reload per notification would only produce redundant work: the
    /// `generation` counter already discards a reload superseded by a newer
    /// one, so anything queued behind the in-flight reload would just be
    /// thrown away the moment it lands.
    fn on_watcher_change(&mut self, cx: &mut Context<Self>) {
        if !self.loading {
            self.reload(cx);
        }
    }

    // -------------------------------------------------------------
    // Detail panel
    // -------------------------------------------------------------

    /// The path of whichever worktree row is currently selected, if any.
    /// Factored out so the Files/Changes tab loading below — which keys its
    /// per-worktree state off the same row — doesn't repeat the lookup.
    fn selected_worktree_path(&self) -> Option<PathBuf> {
        self.selected
            .and_then(|ix| self.rows.get(ix))
            .map(|row| row.path.clone())
    }

    /// Load detail data for whichever row is selected, discarding the
    /// result if the selection has moved on by the time it arrives. A no-op
    /// when the selected path hasn't changed, so the two `apply_rows` passes
    /// of a single `reload` (fast, then with status) don't each kick off
    /// their own redundant load for the same worktree.
    pub(super) fn load_details_for_selection(&mut self, cx: &mut Context<Self>) {
        let path = self.selected_worktree_path();
        if self.details_path == path {
            return;
        }

        self.details_generation += 1;
        let generation = self.details_generation;
        self.details = None;
        self.details_path = path.clone();
        cx.notify();

        // The detail panel's Files/Changes tabs are keyed off this exact
        // same selection change and guarded by this exact same generation
        // counter — see `load_panel_data`'s doc.
        self.load_panel_data(path.clone(), cx);

        let Some(path) = path else {
            return; // Nothing selected: leave the panel showing nothing.
        };
        cx.spawn(async move |this, cx| {
            let details = cx
                .background_spawn(async move { data::worktree_details(&path) })
                .await;
            this.update(cx, |this, cx| this.apply_details(generation, details, cx))
                .ok();
        })
        .detach();
    }

    /// Apply a finished detail load, ignoring one superseded by a newer
    /// selection — the same `generation`-guard shape as `apply_rows`.
    fn apply_details(
        &mut self,
        generation: u64,
        details: Option<WorktreeDetails>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.details_generation {
            return;
        }
        self.details = details;
        cx.notify();
    }

    // -------------------------------------------------------------
    // Detail panel: Files / Changes tabs
    // -------------------------------------------------------------
    //
    // Both tabs' data loads the same way `details` does above, and shares
    // its exact `details_generation` counter rather than keeping a
    // duplicate one: this method only ever runs from the same call site
    // `load_details_for_selection` already gated on "the selection actually
    // changed", so a second, independent counter would move in lockstep
    // with `details_generation` anyway — reusing it is one fewer piece of
    // state that could theoretically drift out of sync with the other.
    //
    // The one load here `details_generation` alone can't guard is the
    // selected file's diff: clicking a different file in the Files tab
    // tree doesn't change the *worktree* selection, so it doesn't bump
    // `details_generation`. `selected_file_diff_key` (a `(worktree, rel
    // path)` pair set synchronously before the load spawns) covers that
    // dimension instead — see `select_tree_file`/`apply_file_diff`.

    /// (Re)prime the Files and Changes tabs for `path`, the worktree that
    /// just became selected (or `None` when nothing is). Ensures the file
    /// tree's root — and any directory the user had previously expanded for
    /// this worktree — is loaded, resumes loading whatever file was
    /// selected in that worktree's tree, and (re)loads the full
    /// `worktree_diff` for the Changes tab.
    fn load_panel_data(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let generation = self.details_generation;

        let Some(path) = path else {
            self.selected_file_diff = SelectedFileDiff::Unselected;
            self.selected_file_diff_key = None;
            self.changes = ChangesState::Loading;
            self.changes_path = None;
            return;
        };

        let tree = self.file_trees.entry(path.clone()).or_default();
        let dirs_to_load = tree.dirs_needing_load();
        for rel_dir in &dirs_to_load {
            tree.set_loading(rel_dir.clone());
        }
        let selected_file = tree.selected_file().map(Path::to_path_buf);
        for rel_dir in dirs_to_load {
            self.spawn_dir_load(path.clone(), rel_dir, generation, cx);
        }

        match selected_file {
            Some(rel) => self.spawn_file_diff_load(path.clone(), rel, generation, cx),
            None => {
                self.selected_file_diff = SelectedFileDiff::Unselected;
                self.selected_file_diff_key = None;
            }
        }

        self.changes = ChangesState::Loading;
        self.changes_path = Some(path.clone());
        self.spawn_changes_load(path, generation, cx);
    }

    fn spawn_dir_load(
        &mut self,
        worktree: PathBuf,
        rel_dir: PathBuf,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let wt = worktree.clone();
            let rel = rel_dir.clone();
            let result = cx
                .background_spawn(async move { data::list_files(&wt, &rel) })
                .await;
            this.update(cx, |this, cx| {
                this.apply_dir_loaded(worktree, rel_dir, generation, result, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Apply a finished directory listing into whichever worktree's tree it
    /// belongs to, ignoring one superseded by a newer selection. Applied
    /// into `file_trees` by worktree path rather than only into "the
    /// currently selected one" — a listing that lands after the user has
    /// already moved on but *before* `details_generation` changed again
    /// (i.e. the same worktree is still selected) should still update that
    /// worktree's cache.
    fn apply_dir_loaded(
        &mut self,
        worktree: PathBuf,
        rel_dir: PathBuf,
        generation: u64,
        result: Result<Vec<data::FileEntry>, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.details_generation {
            return;
        }
        if let Some(tree) = self.file_trees.get_mut(&worktree) {
            match result {
                Ok(entries) => tree.set_loaded(rel_dir, entries),
                Err(e) => tree.set_error(rel_dir, e),
            }
        }
        cx.notify();
    }

    fn spawn_file_diff_load(
        &mut self,
        worktree: PathBuf,
        rel_path: PathBuf,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        self.selected_file_diff = SelectedFileDiff::Loading;
        self.selected_file_diff_key = Some((worktree.clone(), rel_path.clone()));
        let key = (worktree.clone(), rel_path.clone());
        cx.spawn(async move |this, cx| {
            let wt = worktree.clone();
            let rel = rel_path.clone();
            let result = cx
                .background_spawn(async move { data::file_diff(&wt, &rel) })
                .await;
            this.update(cx, |this, cx| {
                this.apply_file_diff(key, generation, result, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Apply a finished single-file diff load. Guarded by both
    /// `details_generation` (a worktree-selection change since this was
    /// spawned) and `selected_file_diff_key` (a different file selected in
    /// the *same* worktree since this was spawned) — see this module's
    /// section doc for why the key is needed in addition to the generation.
    fn apply_file_diff(
        &mut self,
        key: (PathBuf, PathBuf),
        generation: u64,
        result: Result<Option<data::FileDiff>, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.details_generation {
            return;
        }
        if self.selected_file_diff_key.as_ref() != Some(&key) {
            return;
        }
        self.selected_file_diff = match result {
            Ok(Some(diff)) => SelectedFileDiff::Changed(diff),
            Ok(None) => SelectedFileDiff::NoChanges,
            Err(e) => SelectedFileDiff::Error(e),
        };
        cx.notify();
    }

    fn spawn_changes_load(&mut self, worktree: PathBuf, generation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let wt = worktree.clone();
            let result = cx
                .background_spawn(async move { data::worktree_diff(&wt) })
                .await;
            this.update(cx, |this, cx| {
                this.apply_changes(worktree, generation, result, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Apply a finished `worktree_diff` load, ignoring one superseded by a
    /// newer selection — the same `generation`-guard shape as
    /// `apply_details`.
    fn apply_changes(
        &mut self,
        worktree: PathBuf,
        generation: u64,
        result: Result<Vec<data::FileDiff>, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.details_generation {
            return;
        }
        if self.changes_path.as_ref() != Some(&worktree) {
            return;
        }
        self.changes = match result {
            Ok(diffs) => ChangesState::Loaded(diffs),
            Err(e) => ChangesState::Error(e),
        };
        cx.notify();
    }

    // -------------------------------------------------------------
    // Detail panel: Files tab interactions
    // -------------------------------------------------------------

    /// Toggle a directory row's expansion in the currently selected
    /// worktree's tree, kicking off its listing in the background the first
    /// time it's expanded (see `FileBrowserState::dirs_needing_load` — a
    /// re-expand after a collapse reuses the cached listing instead).
    /// A no-op when nothing is selected.
    pub(super) fn toggle_file_dir(&mut self, rel_dir: PathBuf, cx: &mut Context<Self>) {
        let Some(path) = self.selected_worktree_path() else {
            return;
        };
        let generation = self.details_generation;
        let tree = self.file_trees.entry(path.clone()).or_default();
        let now_expanded = tree.toggle_expanded(rel_dir.clone());
        if now_expanded && tree.dir_state(&rel_dir).is_none() {
            tree.set_loading(rel_dir.clone());
            self.spawn_dir_load(path, rel_dir, generation, cx);
        }
        cx.notify();
    }

    /// Select a file in the currently selected worktree's tree, loading its
    /// diff in the background. A no-op when nothing is selected.
    pub(super) fn select_tree_file(&mut self, rel_path: PathBuf, cx: &mut Context<Self>) {
        let Some(path) = self.selected_worktree_path() else {
            return;
        };
        let generation = self.details_generation;
        let tree = self.file_trees.entry(path.clone()).or_default();
        tree.select_file(rel_path.clone());
        self.spawn_file_diff_load(path, rel_path, generation, cx);
        cx.notify();
    }
}

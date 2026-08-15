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
        // Clears any detail data left over from the previous repository —
        // `load_details_for_selection` sees `self.selected` is now `None`
        // and degrades to "nothing loaded" rather than showing a stale
        // worktree's commits under the new repo's name for a frame.
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
                self.rows = rows;
                // A pending selection (set right after a create) wins over
                // the ordinary "keep the previous index in range" rule —
                // but only once: `take()` consumes it so a later manual
                // reload falls back to the normal behavior.
                let pending = self
                    .pending_select
                    .take()
                    .and_then(|branch| self.rows.iter().position(|r| r.display_name() == branch));
                self.selected = pending.or_else(|| match self.selected {
                    // The list is the focus of the window, and an empty
                    // selection makes every keyboard action a no-op.
                    _ if self.rows.is_empty() => None,
                    Some(ix) if ix < self.rows.len() => Some(ix),
                    Some(_) => Some(self.rows.len() - 1),
                    None => Some(0),
                });
                // Rows just changed wholesale (a reload) — a filter that
                // matched some of the old set may match a different subset
                // of the new one, and the pending-selection branch above
                // can itself have picked a now-hidden row; both are why
                // this runs *after* it rather than folding into it.
                self.clamp_selection_to_filter(cx);
                self.sync_watcher(cx);
                self.load_details_for_selection(cx);
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

    /// Load detail data for whichever row is selected, discarding the
    /// result if the selection has moved on by the time it arrives. A no-op
    /// when the selected path hasn't changed, so the two `apply_rows` passes
    /// of a single `reload` (fast, then with status) don't each kick off
    /// their own redundant load for the same worktree.
    pub(super) fn load_details_for_selection(&mut self, cx: &mut Context<Self>) {
        let path = self
            .selected
            .and_then(|ix| self.rows.get(ix))
            .map(|row| row.path.clone());
        if self.details_path == path {
            return;
        }

        self.details_generation += 1;
        let generation = self.details_generation;
        self.details = None;
        self.details_path = path.clone();
        cx.notify();

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
}

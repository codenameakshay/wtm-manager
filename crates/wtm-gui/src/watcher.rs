//! Live filesystem watching for the active repository.
//!
//! `wtm`'s state (worktree list, branch, dirty/ahead/behind) can change
//! entirely outside the app — a commit in a terminal, a branch switch in an
//! editor, another tool creating a worktree. [`RepoWatcher`] notices that on
//! its own instead of waiting for the user to press ⌘R.
//!
//! # The central design decision: what gets watched recursively
//!
//! - The repository's git directory (`ctx.git_dir`) is watched **recursively
//!   as a single root**. `HEAD`, `refs/`, `index`, and `worktrees/` all live
//!   directly under it, so one watch descriptor covers branch switches,
//!   commits, staging, and worktree add/remove. It also picks up
//!   `objects/` and `logs/`, which churn on every write without changing
//!   anything the app renders — those are dropped by [`is_relevant_change`]
//!   rather than by narrowing the watch to four separate roots, which would
//!   trade one watch descriptor (and one failure mode) for four.
//! - Each worktree's own root directory, plus its `.git` entry, is watched
//!   **non-recursively**. Recursing into a working tree would watch every
//!   build artifact, every `node_modules` write, and every editor swap file
//!   it contains — expensive on a large repo and noisy enough to trigger a
//!   refresh loop while a build runs. A non-recursive watch still catches
//!   top-level changes and, more importantly, the `.git` entry itself
//!   (relevant when a worktree is relocated or its bound branch data is
//!   touched directly). Anything deeper in a worktree is covered indirectly:
//!   branch/HEAD state comes from the git-dir watch above, and a full status
//!   recompute still happens on every manual ⌘R.
//!
//! # Constraints this module works under
//!
//! - gpui is single-threaded on the UI side: an `Entity` may not be touched
//!   from another OS thread. The debouncer's background thread only ever
//!   sends a unit signal down a [`std::sync::mpsc`] channel; a gpui
//!   foreground task drains it and is the only thing that calls back into
//!   the view.
//! - Watching can fail (missing directory, permissions, platform
//!   watch-descriptor limits). None of that may panic — manual ⌘R refresh
//!   has to keep working regardless — so every failure degrades to "watch
//!   less" rather than propagating an error.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{AsyncApp, Context, Task, WeakEntity};
use notify_debouncer_full::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};

/// The interval within which a burst of filesystem events collapses into a
/// single notification. A `git commit` or `git worktree add` touches many
/// files as one logical operation; without debouncing, each of those writes
/// would trigger its own refresh.
const DEBOUNCE_WINDOW: Duration = Duration::from_millis(400);

/// Watches a repository's git directory and worktrees, invoking a callback
/// on the gpui foreground whenever the repository may have changed.
///
/// Holds the debouncer (background thread + OS watch descriptors) and the
/// foreground task draining its notifications. Dropping a `RepoWatcher` —
/// directly, or by rebinding it via [`RepoWatcher::watch`] — drops both:
/// the debouncer's `Drop` stops its thread, and a gpui `Task` is cancelled
/// the instant it is dropped, so nothing keeps running unobserved.
pub struct RepoWatcher {
    debouncer: Option<Debouncer<RecommendedWatcher, RecommendedCache>>,
    drain_task: Option<Task<()>>,
}

impl RepoWatcher {
    /// Start watching `git_dir` plus `worktrees`, invoking `on_change` on the
    /// gpui foreground (coalesced) whenever the repository may have changed.
    ///
    /// Returns `None` when nothing could be watched at all (see
    /// [`RepoWatcher::watch`]) — the caller should treat that the same as
    /// "live refresh unavailable, manual reload still works", not as an
    /// error to surface.
    pub fn new<T: 'static>(
        git_dir: PathBuf,
        worktrees: Vec<PathBuf>,
        cx: &mut Context<T>,
        on_change: impl Fn(&mut T, &mut Context<T>) + 'static,
    ) -> Option<Self> {
        let mut this = Self {
            debouncer: None,
            drain_task: None,
        };
        this.watch(git_dir, worktrees, cx, on_change)
            .then_some(this)
    }

    /// (Re)target the watch at a different repository or worktree set.
    ///
    /// This is how rebinding works — there is no separate "stop" method.
    /// The previous debouncer and drain task (if any) are released *before*
    /// new ones are created, so their OS watch descriptors and background
    /// thread are freed first; this matters on platforms with a small
    /// per-process watch limit (inotify's default is in the low thousands).
    ///
    /// Returns whether anything ended up being watched. On `false` the
    /// watcher is left inert rather than pointed at stale paths — callers
    /// that get `false` from here (or `None` from `new`) keep working via
    /// manual reload, just without live updates.
    pub fn watch<T: 'static>(
        &mut self,
        git_dir: PathBuf,
        worktrees: Vec<PathBuf>,
        cx: &mut Context<T>,
        on_change: impl Fn(&mut T, &mut Context<T>) + 'static,
    ) -> bool {
        self.debouncer = None;
        self.drain_task = None;

        let Some((debouncer, rx)) = start_watching(&git_dir, &worktrees) else {
            return false;
        };

        // The consumer side: block on `rx.recv()` on a background executor
        // thread (blocking there is fine — it's the same pattern `data::`
        // calls use for git2/process work), then hop to the foreground to
        // run `on_change`. The receiver is threaded back out of each
        // background hop so the next iteration can reuse it.
        let drain_task = cx.spawn(async move |handle: WeakEntity<T>, cx: &mut AsyncApp| {
            let mut rx = rx;
            loop {
                let (item, returned_rx) = cx
                    .background_spawn(async move {
                        let item = rx.recv();
                        (item, rx)
                    })
                    .await;
                rx = returned_rx;

                if item.is_err() {
                    // The sender lives inside the debouncer's event-handler
                    // closure; it being gone means the debouncer was
                    // dropped (this watcher was rebound or torn down), so
                    // nothing more will ever arrive.
                    break;
                }

                // Coalesce: absorb any further notifications that queued up
                // while this task was asleep or busy running `on_change`,
                // so a run of debounced batches collapses into one refresh
                // instead of one per batch.
                drain_pending(&rx);

                if handle.update(cx, |view, cx| on_change(view, cx)).is_err() {
                    // The view has been released; no one left to notify.
                    break;
                }
            }
        });

        self.debouncer = Some(debouncer);
        self.drain_task = Some(drain_task);
        true
    }
}

/// Wire up the debounced watch and start forwarding filtered change signals
/// on the returned channel. Returns `None` when the debouncer itself
/// couldn't be created, or when not a single path ended up watched.
///
/// Split out from [`RepoWatcher::watch`] so the effectful watching,
/// debouncing, and filtering pipeline — the part actually worth exercising
/// with real files — can be tested independent of gpui: nothing here
/// touches an `Entity` or needs a running `App`.
fn start_watching(
    git_dir: &Path,
    worktrees: &[PathBuf],
) -> Option<(
    Debouncer<RecommendedWatcher, RecommendedCache>,
    mpsc::Receiver<()>,
)> {
    let (tx, rx) = mpsc::channel();

    let mut debouncer = new_debouncer(DEBOUNCE_WINDOW, None, move |result: DebounceEventResult| {
        let relevant = match &result {
            Ok(events) => events
                .iter()
                .any(|event| event.paths.iter().any(|path| is_relevant_change(path))),
            // A watch error (e.g. the OS event queue overflowed) means
            // events may have been missed; treat that as relevant so the
            // app resyncs instead of silently drifting stale.
            Err(_) => true,
        };
        if relevant {
            // Fails only once the receiving end (the drain task) is gone,
            // at which point the debouncer is on its way out too and the
            // signal has nowhere useful to go.
            let _ = tx.send(());
        }
    })
    .ok()?;

    let mut watched_anything = watch_git_dir(&mut debouncer, git_dir);
    for worktree in worktrees {
        watched_anything |= watch_worktree(&mut debouncer, git_dir, worktree);
    }

    watched_anything.then_some((debouncer, rx))
}

/// Drain any notifications already queued behind the one just received. See
/// the "central design decision" module docs for why the git-dir watch is
/// recursive and needs this: a burst of separate debounced batches
/// completing faster than the foreground can keep up would otherwise run
/// `on_change` once per batch instead of once for the whole burst.
fn drain_pending(rx: &mpsc::Receiver<()>) {
    while rx.try_recv().is_ok() {}
}

/// Recursively watch the whole git directory. See the module-level "central
/// design decision" docs for why this is one recursive watch relying on
/// [`is_relevant_change`] to filter noise, rather than several narrow ones.
fn watch_git_dir(
    debouncer: &mut Debouncer<RecommendedWatcher, RecommendedCache>,
    git_dir: &Path,
) -> bool {
    watch_path(debouncer, git_dir, RecursiveMode::Recursive)
}

/// Watch one worktree's root directory non-recursively, plus its own `.git`
/// entry (a file for a linked worktree, a directory for the main one). See
/// the module-level docs for why this stops short of a recursive watch.
fn watch_worktree(
    debouncer: &mut Debouncer<RecommendedWatcher, RecommendedCache>,
    git_dir: &Path,
    root: &Path,
) -> bool {
    let mut watched = watch_path(debouncer, root, RecursiveMode::NonRecursive);

    let dot_git = root.join(".git");
    // For the main worktree, `.git` *is* `git_dir` — already covered by
    // `watch_git_dir` above, so watching it again would just be a redundant
    // descriptor for the same directory.
    if dot_git.as_path() != git_dir {
        watched |= watch_path(debouncer, &dot_git, RecursiveMode::NonRecursive);
    }

    watched
}

/// Attempt to watch one path, degrading to `false` instead of propagating an
/// error. A missing worktree directory, a permissions error, or (macOS/
/// Linux) exhausting the platform's watch-descriptor limit are all things a
/// user can hit in the wild, and none of them should keep the app from
/// working — only from refreshing itself automatically. Manual ⌘R does not
/// depend on this.
fn watch_path(
    debouncer: &mut Debouncer<RecommendedWatcher, RecommendedCache>,
    path: &Path,
    mode: RecursiveMode,
) -> bool {
    match debouncer.watch(path, mode) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("wtm: could not watch {}: {err}", path.display());
            false
        }
    }
}

/// Whether a changed path is signal the app should react to, as opposed to
/// noise that notify-debouncer-full would otherwise forward on every commit,
/// checkout, or background build. A pure function so the ignore rules are
/// testable without any real watching.
///
/// Three classes of noise are dropped:
/// - `.git/objects/**` — every commit, `git add`, and `git gc` writes here;
///   it's git's content-addressed blob store, not something the app shows.
///   What matters is the refs that come to point into it, which are *not*
///   filtered.
/// - lock files (any filename ending `.lock`, notably `index.lock`) — git
///   creates and removes these around nearly every write as a mutual-
///   exclusion marker. They are the *announcement* of a change, not the
///   change itself; reacting to their creation risks reading state
///   mid-write (e.g. a half-updated index).
/// - `.git/logs/**` (the reflog) — appended to on every ref update, i.e. on
///   every event that already triggers a refresh via `HEAD` or `refs/`.
///   Watching it too would fire the callback twice per operation for no new
///   information.
fn is_relevant_change(path: &Path) -> bool {
    let is_lock_file = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".lock"));
    if is_lock_file {
        return false;
    }

    let components: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    !components
        .windows(2)
        .any(|pair| pair == [".git", "objects"] || pair == [".git", "logs"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc::RecvTimeoutError;

    // -- is_relevant_change: pure, no filesystem or watching involved --

    #[test]
    fn accepts_a_ref_update() {
        assert!(is_relevant_change(Path::new("refs/heads/main")));
        assert!(is_relevant_change(Path::new(
            "/repo/.git/refs/heads/feature"
        )));
    }

    #[test]
    fn accepts_head_and_worktrees_dir() {
        assert!(is_relevant_change(Path::new("/repo/.git/HEAD")));
        assert!(is_relevant_change(Path::new(
            "/repo/.git/worktrees/feature/HEAD"
        )));
        assert!(is_relevant_change(Path::new("/repo/.git/index")));
    }

    #[test]
    fn rejects_object_database_writes() {
        assert!(!is_relevant_change(Path::new(
            "/repo/.git/objects/ab/cdef0123456789"
        )));
    }

    #[test]
    fn rejects_lock_files() {
        assert!(!is_relevant_change(Path::new("/repo/.git/index.lock")));
        assert!(!is_relevant_change(Path::new("index.lock")));
        assert!(!is_relevant_change(Path::new(
            "/repo/.git/refs/heads/main.lock"
        )));
    }

    #[test]
    fn rejects_reflog_writes() {
        assert!(!is_relevant_change(Path::new("/repo/.git/logs/HEAD")));
        assert!(!is_relevant_change(Path::new(
            "/repo/.git/logs/refs/heads/main"
        )));
    }

    // -- drain_pending: pure channel logic, no watching involved --

    #[test]
    fn drain_pending_collapses_queued_notifications() {
        let (tx, rx) = mpsc::channel();
        tx.send(()).unwrap();
        tx.send(()).unwrap();
        tx.send(()).unwrap();

        // Simulate the drain loop: take the first item like `rx.recv()`
        // would, then let `drain_pending` absorb the rest.
        assert_eq!(rx.try_recv(), Ok(()));
        drain_pending(&rx);

        assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty));
    }

    // -- start_watching: real files, real notify/notify-debouncer-full --

    #[test]
    fn watching_git_dir_reports_a_relevant_change() {
        let tmp = tempfile::TempDir::new().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::create_dir_all(git_dir.join("objects")).unwrap();

        let (_debouncer, rx) = start_watching(&git_dir, &[]).expect("watch should succeed");

        fs::write(git_dir.join("refs/heads/main"), b"deadbeef\n").unwrap();

        // Generous timeout: debounce is 400ms, and some backends (macOS
        // FSEvents in particular) add their own startup latency on top.
        rx.recv_timeout(Duration::from_secs(5))
            .expect("a relevant change under refs/ should produce a notification");
    }

    #[test]
    fn watching_git_dir_ignores_object_writes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(git_dir.join("objects/ab")).unwrap();

        let (_debouncer, rx) = start_watching(&git_dir, &[]).expect("watch should succeed");

        fs::write(git_dir.join("objects/ab/cdef0123456789"), b"blob").unwrap();

        // No message should ever arrive for a filtered path, so waiting
        // comfortably past the debounce window and finding nothing is not
        // timing-sensitive the way asserting presence would be.
        let result = rx.recv_timeout(Duration::from_millis(1500));
        assert_eq!(result, Err(RecvTimeoutError::Timeout));
    }

    #[test]
    fn missing_directory_degrades_to_none_instead_of_failing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let missing = tmp.path().join("does-not-exist").join(".git");

        assert!(start_watching(&missing, &[]).is_none());
    }
}

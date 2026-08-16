//! Headless integration tests for the whole app, driven through gpui's
//! `test-support` harness (`TestAppContext`/`#[gpui::test]`) instead of
//! manual launching or screen capture, neither of which is reliable in this
//! environment.
//!
//! ## Why this module lives here
//!
//! `wtm-gui` is a binary crate (`src/main.rs`), so a top-level `tests/`
//! directory cannot see any of its modules — only `main.rs` itself is ever
//! compiled as a crate root for an external integration test, and it has no
//! `pub` surface to speak of. The alternative the task brief names,
//! `src/integration_tests.rs` declared from `main.rs`, would only reach
//! `WtmApp`'s `pub`/`pub(crate)` surface: nearly everything this suite needs
//! to assert on (`rows`, `selected`, `multi_selected`, `dialog`, `palette`,
//! `bulk_remove`, `active`, `loading`, …) is a *private* field of `WtmApp`,
//! and Rust's privacy rule for a private item is "visible in the defining
//! module and its descendants" — `main`'s hypothetical `integration_tests`
//! would be a *sibling* of `app`, not a descendant, and so could not see any
//! of it. Declaring this module as a child of `app` instead
//! (`app/mod.rs`'s `#[cfg(test)] mod integration_tests;`) makes it a real
//! descendant of every module that defines `WtmApp`'s state, so tests can
//! read that state directly rather than reconstructing it from rendered
//! output — and can drive the exact `pub(super)`/`pub(crate)` handler
//! methods a click or keystroke would call (see "Mouse-driven flows" below).
//!
//! ## Mouse-driven flows
//!
//! gpui's `TestWindow` can simulate raw mouse events, but resolving them to
//! a specific on-screen row requires `Stateful::debug_selector`, a hook the
//! app's rendering code does not currently call anywhere (`worktree_list`'s
//! rows live inside a virtualized `uniform_list`, so there is no pixel
//! geometry to click on without it). Retrofitting that instrumentation
//! throughout the row/button rendering code for every flow this suite
//! covers would be a far larger, more invasive change than this task's
//! "keep the refactor minimal" instruction allows. Flows that are only
//! reachable with the mouse in the real app (⌘-click / shift-click / a
//! checkbox click for multi-select; a dialog's toggle/confirm buttons) are
//! instead driven by calling the exact `pub(super)` handler method the
//! click would invoke (`toggle_row_selection`, `extend_selection_range`,
//! `confirm_remove_dialog`, `toggle_prune_merged`, …) via
//! `Entity::update_in`. This exercises the real production logic those
//! click handlers delegate to — only the "which pixel maps to which
//! handler" glue (a few lines per call site, already exhaustively covered
//! by `cargo build`'s own type checking of the closures in `chrome.rs`) is
//! left untested. Anywhere an action has a real keybinding (⌘N, ⌘⌫, ⌘⇧P,
//! ⌘F, ⌘K, ⌘I, ⌘1/2/3, ⌘⇧O, Escape, …) these tests use
//! `cx.simulate_keystrokes`/`simulate_input` instead, going through the
//! exact same `KeyBinding` table `main.rs` registers for the real app (see
//! `open_app` below).
//!
//! ## What could not be driven headlessly at all
//!
//! "Add Repository" (⌘⇧O) opens a native folder picker via
//! `cx.prompt_for_paths`. The task brief suggested
//! `cx.simulate_new_path_selection` drives this, but reading gpui 0.2.2's
//! own `TestPlatform` (`platform/test/platform.rs`) shows that helper only
//! ever answers `prompt_for_new_path` (a *save* dialog) — its
//! `prompt_for_paths` (the *open/choose existing* dialog `AddRepository`
//! actually calls, `directories: true`) is `unimplemented!()` and panics
//! unconditionally if invoked. There is no test-harness hook for it in this
//! gpui version. `WtmApp::finish_add_repository` — the resolve-and-activate
//! logic that runs once the picker returns a path — was bumped from private
//! to `pub(super)` (see `app/commands.rs`) so its real logic can still be
//! exercised directly; only the platform picker call itself is untested,
//! and could not be, short of a much larger vendoring/mocking effort this
//! task's "minimal refactor" instruction rules out.
//!
//! The live filesystem watcher (`crate::watcher::RepoWatcher`) is not
//! exercised either: it is driven by real OS filesystem-change
//! notifications on a background thread, which is exactly the kind of
//! non-deterministic input `run_until_parked`/`advance_clock` cannot make
//! reproducible — every flow that watcher would eventually trigger (a
//! reload) is instead tested by triggering that same `reload` deterministic
//! through the app's own actions (⌘R, a create/remove/prune completing).
//! Worse: starting a *real* watcher inside a `#[gpui::test]` hangs
//! `run_until_parked` forever (`RepoWatcher::watch`'s consumer task blocks
//! on `rx.recv()`, which is fine on the real app's dedicated background
//! thread but fatal under `TestDispatcher`'s single-threaded, cooperative
//! model — see `crate::watcher::DISABLED_FOR_TESTS`'s doc comment for the
//! full explanation). Since opening a repository always starts one
//! (`WtmApp::apply_rows` -> `sync_watcher`, unconditionally), every test in
//! this module calls `disable_watcher_for_tests()` before opening a window.
//!
//! ## Isolation
//!
//! The app reads and writes `$WTM_CONFIG_DIR/repos.json` and `gui.json` on
//! essentially every meaningful action (`WtmApp::new` alone calls
//! `registry::load()` unconditionally). Every test here runs inside a
//! [`Fixture`], which points `WTM_CONFIG_DIR` at a fresh temp directory for
//! its whole lifetime and never touches `~/.config/wtm`. Env vars are
//! process-global, so `Fixture` serializes on `crate::prefs::ENV_LOCK` —
//! the exact same lock `prefs`'s own tests already use for the same
//! variable, relocated to crate visibility (see that module) specifically
//! so the two test suites can't race each other under `cargo test`'s
//! default parallelism.
//!
//! Nothing here writes outside a `tempfile::TempDir`, spawns a real Finder
//! window, launches a real terminal, or touches the system clipboard —
//! `reveal_in_finder`/`open_in_terminal`/`copy_to_clipboard` are
//! deliberately not exercised for that reason (they are not part of any of
//! the twelve required flows either).
//!
//! ## Determinism
//!
//! `submit_create_dialog`'s progress view drains a channel from a polling
//! loop (a 16ms delay between checks), so a create is only driven to
//! completion with `cx.executor().advance_clock(...)` after
//! `run_until_parked` — see `TestDispatcher::advance_clock`'s doc: it makes
//! due timers ready and re-drains ready work in a loop, which is what lets
//! a bounded, single call unstick an arbitrary number of 16ms poll ticks
//! deterministically rather than guessing how many are needed. That delay
//! **must** be `cx.background_executor().timer(..)`, not `gpui::Timer`
//! (`smol::Timer::after`, a raw wall-clock timer gpui merely re-exports):
//! the latter bypasses the platform dispatcher entirely, so
//! `TestDispatcher` — and therefore `advance_clock` — cannot see or control
//! it at all. This was a real, previously-latent bug in
//! `submit_create_dialog` (and in `TextInput::start_blinking`, fixed the
//! same way): found because a *fast* create (an early validation failure,
//! no real `git worktree add` on the critical path) reliably hung waiting
//! on genuine, un-simulatable wall-clock time, while a *slow* one (a real
//! git subprocess call) happened to pass by accident, once enough real time
//! had elapsed incidentally elsewhere in the test. Both call sites now use
//! the dispatcher-integrated timer; see `app/dialog_actions.rs` and
//! `text_input.rs` for the fix and its full explanation.
//!
//! Every other background operation here (reload, remove, prune, bulk
//! remove, add repository) is a single `background_spawn().await` with no
//! artificial delay, so a plain `run_until_parked` (already implied by
//! `simulate_keystrokes`/`simulate_input`) is enough to settle it — with
//! one exception: `run_until_parked` *always* drains everything currently
//! ready, including a `finish_prune_dialog`/`finish_bulk_remove`-style
//! handler's own follow-up `WtmApp::reload`. That reload's with-status pass
//! clears a purely informational (non-error) status message the instant it
//! lands (see `WtmApp::apply_rows`), so asserting on such a status *after*
//! a full `run_until_parked` races against, and reliably loses to, the very
//! refresh the operation itself triggers. `run_until` (below) stops the
//! instant a predicate is satisfied instead, before that follow-up reload
//! gets a chance to run.

use std::process::Command as StdCommand;
use std::time::Duration;

use gpui::{TestAppContext, VisualTestContext};
use tempfile::TempDir;

use super::*;

// ---------------------------------------------------------------------
// Fixture plumbing
// ---------------------------------------------------------------------

/// RAII guard: sets `WTM_CONFIG_DIR` under `crate::prefs::ENV_LOCK`,
/// restores on drop. See this module's doc comment on why the lock is
/// shared with `prefs`'s own tests rather than a second, independent one.
struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn set(dir: &Path) -> Self {
        let lock = prefs::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("WTM_CONFIG_DIR", dir);
        EnvGuard { _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        std::env::remove_var("WTM_CONFIG_DIR");
    }
}

/// Run a fixture-setup git command hermetically (fixed identity, no
/// signing, no host global config), panicking on failure. Mirrors the `git`
/// test helper `crate::data`'s own tests already use for the same purpose.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = StdCommand::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "wtm-gui test")
        .env("GIT_AUTHOR_EMAIL", "wtm-gui@example.invalid")
        .env("GIT_COMMITTER_NAME", "wtm-gui test")
        .env("GIT_COMMITTER_EMAIL", "wtm-gui@example.invalid")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("failed to run git");
    assert!(
        out.status.success(),
        "git {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A real, throwaway git repository plus an isolated `WTM_CONFIG_DIR`, torn
/// down together when dropped. Never touches a developer's real
/// repositories or `~/.config/wtm` — see this module's doc comment.
///
/// Baseline layout, built once by [`Fixture::new`]:
/// - `main`, one seed commit.
/// - `develop`, a second local branch advanced one commit *beyond* `main`
///   via a scratch worktree that is then removed — so it is a real,
///   resolvable, distinct base for the create dialog's base-ref picker,
///   without ever being checked out anywhere lasting.
/// - a fake remote-tracking `refs/remotes/origin/main` (same trick
///   `crate::data`'s own `list_refs_end_to_end_against_a_real_repo` test
///   uses — no real remote is needed to exercise that code path).
/// - one linked worktree on branch `feature-x`, with an uncommitted,
///   untracked file (dirty).
///
/// [`Fixture::add_worktree`] grows additional linked worktrees (each
/// trivially merged into `main`, since they start from its current tip)
/// for tests that need more than this baseline.
struct Fixture {
    _tmp: TempDir,
    _env: EnvGuard,
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().expect("create tempdir");
        // Canonicalize up front so every derived path compares equal on
        // macOS (/tmp vs /private/tmp) — same reasoning as the root crate's
        // `tests/common/mod.rs`.
        let base = tmp.path().canonicalize().expect("canonicalize tempdir");
        let wtm_config = base.join("wtm-config");
        std::fs::create_dir_all(&wtm_config).expect("create wtm-config dir");
        let env = EnvGuard::set(&wtm_config);

        let root = base.join("repo");
        std::fs::create_dir_all(&root).expect("create repo dir");
        git(&root, &["init", "-b", "main"]);
        std::fs::write(root.join("README.md"), "seed\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "seed"]);

        let fx = Fixture {
            _tmp: tmp,
            _env: env,
            base,
            root,
        };

        git(&fx.root, &["branch", "develop"]);
        fx.advance_branch("develop", "develop.txt", "develop work\n");
        git(
            &fx.root,
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
        );

        let feature_x = fx.add_worktree("feature-x");
        fx.write_untracked(&feature_x, "scratch.txt", "uncommitted\n");

        fx
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        git(dir, args)
    }

    /// Where [`add_worktree`](Self::add_worktree) puts a worktree for
    /// `branch` — usable even after the directory has been deleted (e.g. to
    /// simulate a `rm -rf`'d worktree), unlike a path recovered via
    /// `canonicalize`.
    fn worktree_path(&self, branch: &str) -> PathBuf {
        self.base.join("repo-worktrees").join(branch)
    }

    /// Add a linked worktree on a brand new branch, from `main`'s current
    /// tip — trivially merged into it (identical commit), and clean until
    /// the caller writes something into it.
    fn add_worktree(&self, branch: &str) -> PathBuf {
        let path = self.worktree_path(branch);
        git(
            &self.root,
            &["worktree", "add", path.to_str().unwrap(), "-b", branch],
        );
        path
    }

    /// Advance `branch` by one commit without leaving a lasting worktree on
    /// it: a scratch worktree is created, committed into, then removed —
    /// `branch` survives as a plain, unattached local branch with a tip
    /// distinct from `main`'s. Returns the new tip's full sha.
    fn advance_branch(&self, branch: &str, filename: &str, contents: &str) -> String {
        let scratch = self.base.join(format!("scratch-{branch}"));
        git(
            &self.root,
            &["worktree", "add", scratch.to_str().unwrap(), branch],
        );
        std::fs::write(scratch.join(filename), contents).unwrap();
        git(&scratch, &["add", "."]);
        git(&scratch, &["commit", "-m", &format!("advance {branch}")]);
        let sha = git(&scratch, &["rev-parse", "HEAD"]);
        git(
            &self.root,
            &["worktree", "remove", "--force", scratch.to_str().unwrap()],
        );
        sha
    }

    fn write_untracked(&self, dir: &Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).expect("write untracked file");
    }

    fn branch_exists(&self, name: &str) -> bool {
        !git(&self.root, &["branch", "--list", name]).is_empty()
    }

    /// The library's own worktree listing (not the app's `rows`) — used to
    /// assert against the real repository, independent of anything the app
    /// might have gotten wrong.
    fn list_worktrees(&self) -> Vec<WorktreeInfo> {
        let ctx = wtm::repo::discover(Some(&self.root)).expect("discover repo");
        wtm::worktree::list(
            &ctx,
            &wtm::worktree::ListOptions {
                with_status: true,
                base: None,
            },
        )
        .expect("list worktrees")
    }

    /// Open this fixture's repository the same way `main.rs`/the sidebar
    /// does.
    fn open(&self) -> OpenRepo {
        data::open_repo(&self.root).expect("open fixture repo")
    }

    /// Build a second, independent repository under this fixture's own
    /// scratch space — for the Add Repository flow, which needs more than
    /// one repository to exist. Deliberately does not touch
    /// `WTM_CONFIG_DIR` itself: the owning `Fixture` already holds that for
    /// the whole test, and `EnvGuard::set`'s lock is not reentrant.
    fn sibling_repo(&self, name: &str) -> PathBuf {
        let root = self.base.join(name);
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        std::fs::write(root.join("README.md"), "seed\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "seed"]);
        root
    }

    /// A plain directory that is not a git repository at all.
    fn non_repo_dir(&self, name: &str) -> PathBuf {
        let dir = self.base.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}

/// Bind the app's real keymap (the same `key_bindings!` table `main.rs`
/// installs) and open a window on `initial`, running the harness to a
/// parked state the same way the real window's first paint would settle.
/// Shadowing the returned `cx` at the call site
/// (`let (view, cx) = open_app(cx, ...)`) is the standard gpui pattern —
/// see `TestAppContext::add_window_view`'s own doc comment.
fn open_app(
    cx: &mut TestAppContext,
    initial: Option<OpenRepo>,
) -> (Entity<WtmApp>, &mut VisualTestContext) {
    disable_watcher_for_tests();
    cx.update(|cx| {
        theme::init(cx);
        cx.bind_keys(crate::registered_key_bindings());
    });
    let (view, cx) =
        cx.add_window_view(|window, cx| WtmApp::new(initial, Prefs::default(), window, cx));
    // Mirrors `main.rs`'s own `window.activate_window()` call, made right
    // after opening the real window. Without it, gpui's window-level
    // `window_active` flag stays false for the lifetime of the test, and
    // `Window::draw` unconditionally reports an *empty* current-focus path
    // to every `cx.on_focus`/`cx.on_blur` listener whenever the window
    // isn't active (see `window.rs`'s frame-diffing in `draw`) — regardless
    // of what `window.focus(..)` actually set. `CreateState`'s base-ref
    // picker (`open_base_picker`/`close_base_picker`) is wired entirely
    // through such a listener, so without this call it would never open in
    // a test no matter how focus is driven, while working perfectly for a
    // real user, whose window is always active by the time they can click
    // anything. This is a test-harness gap, not an app bug: verified by
    // reading gpui 0.2.2's own `Window::draw` (the `window_active` gate)
    // and confirming `main.rs` already performs the equivalent activation
    // at real startup, before any user interaction is possible.
    cx.update(|window, _| window.activate_window());
    (view, cx)
}

/// Tick the dispatcher one ready task at a time until `predicate` holds on
/// `view`'s state, then stop — deliberately *not* `cx.run_until_parked()`,
/// which always drains everything currently ready. A `finish_prune_dialog`/
/// `finish_bulk_remove`-style handler sets a status message and then
/// immediately calls `WtmApp::reload` in the same synchronous step; a full
/// `run_until_parked` therefore also drains that reload's own with-status
/// pass, which — by design (see `WtmApp::apply_rows`) — clears a purely
/// informational (non-error) status the moment it lands, since nothing
/// stops it from being superseded by a normal refresh. Asserting on the
/// status text after a full `run_until_parked` races against, and reliably
/// loses to, the very refresh the operation itself triggers. This stops the
/// instant `predicate` is satisfied, before that follow-up reload gets a
/// chance to run. Panics (with a clear message, not a hang) if the
/// dispatcher fully parks without ever satisfying `predicate`.
fn run_until<T: 'static>(
    cx: &mut VisualTestContext,
    view: &Entity<T>,
    mut predicate: impl FnMut(&T) -> bool,
) {
    while !view.read_with(cx, |v, _| predicate(v)) {
        assert!(
            cx.executor().tick(),
            "dispatcher fully parked before the expected state was ever reached"
        );
    }
}

/// See `crate::watcher::DISABLED_FOR_TESTS`'s doc comment: a real
/// `RepoWatcher` hangs `run_until_parked` forever inside
/// `TestAppContext`'s single-threaded dispatcher, and opening a repository
/// always starts one (`WtmApp::apply_rows` -> `sync_watcher`,
/// unconditionally). Every test in this module that opens a window must
/// call this first. Idempotent, so it is safe to call once per test with no
/// matching "re-enable" — no other test module ever constructs a `WtmApp`.
fn disable_watcher_for_tests() {
    crate::watcher::DISABLED_FOR_TESTS.store(true, std::sync::atomic::Ordering::Relaxed);
}

// ---------------------------------------------------------------------
// 1. Startup
// ---------------------------------------------------------------------

#[gpui::test]
fn startup_seeds_rows_synchronously_then_settles_status(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();

    disable_watcher_for_tests();
    cx.update(|cx| {
        theme::init(cx);
        cx.bind_keys(crate::registered_key_bindings());
    });
    // The lower-level `add_window` (unlike `add_window_view`) does not
    // itself run the executor, so reading state immediately afterward
    // really does inspect the state from before any background task has
    // ever had a chance to run — exactly the "the first render already has
    // rows" claim `WtmApp::seed_initial_rows` makes.
    let window = cx.add_window(|window, cx| WtmApp::new(Some(repo), Prefs::default(), window, cx));

    let seeded = window
        .update(cx, |app, _window, _cx| app.rows.len())
        .unwrap();
    assert_eq!(
        seeded, 2,
        "the synchronous seed must already list every worktree (main + feature-x)"
    );

    cx.run_until_parked();

    let view = window.root(cx).unwrap();
    view.read_with(cx, |app, _| {
        assert!(!app.loading, "loading must resolve to false");
        assert!(
            !app.awaiting_status,
            "the with-status pass must have landed"
        );
        assert_eq!(app.rows.len(), 2);
        assert!(app.rows.iter().any(|r| r.is_main));
        let feature = app
            .rows
            .iter()
            .find(|r| r.display_name() == "feature-x")
            .expect("feature-x row");
        assert!(
            feature.status.as_ref().unwrap().dirty,
            "feature-x has an uncommitted file"
        );
    });

    assert_eq!(fx.list_worktrees().len(), 2, "matches the real repository");
}

// ---------------------------------------------------------------------
// 2. Create worktree
// ---------------------------------------------------------------------

#[gpui::test]
fn create_worktree_via_shortcut_creates_on_disk_and_grows_rows(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    let before = view.read_with(cx, |app, _| app.rows.len());

    cx.simulate_keystrokes("cmd-n");
    view.read_with(cx, |app, _| {
        assert!(
            matches!(app.dialog, Some(Dialog::Create(_))),
            "cmd-n opens the create dialog"
        );
    });

    cx.simulate_input("brand-new-feature");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(2));

    view.read_with(cx, |app, _| {
        let Some(Dialog::Create(state)) = &app.dialog else {
            panic!("dialog should still be open, showing the progress phase");
        };
        let CreatePhase::Progress(progress) = &state.phase else {
            panic!("expected the progress phase after submitting");
        };
        match progress
            .outcome
            .as_ref()
            .expect("create should have finished")
        {
            Ok(_) => {}
            Err(e) => panic!("create failed: {e}"),
        }
    });

    let after = view.read_with(cx, |app, _| app.rows.len());
    assert_eq!(after, before + 1, "the row set must have grown by one");

    let path = fx.worktree_path("brand-new-feature");
    assert!(path.is_dir(), "the worktree must exist on disk");
    assert!(
        fx.list_worktrees()
            .iter()
            .any(|w| w.branch.as_deref() == Some("brand-new-feature")),
        "git's own registry must list it"
    );

    view.read_with(cx, |app, _| {
        let selected = app.selected.and_then(|ix| app.rows.get(ix));
        assert_eq!(
            selected.map(|r| r.display_name()),
            Some("brand-new-feature"),
            "the just-created worktree must end up selected"
        );
    });
}

// ---------------------------------------------------------------------
// 3. Create validation
// ---------------------------------------------------------------------

#[gpui::test]
fn create_rejects_branch_checked_out_elsewhere(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    let before_rows = view.read_with(cx, |app, _| app.rows.len());
    let before_worktrees = fx.list_worktrees().len();

    cx.simulate_keystrokes("cmd-n");
    view.read_with(cx, |app, _| {
        let Some(Dialog::Create(state)) = &app.dialog else {
            panic!("dialog must be open");
        };
        let feature = state
            .branches
            .iter()
            .find(|b| b.name == "feature-x")
            .expect("the branch picker must list feature-x");
        assert!(
            feature.is_checked_out,
            "feature-x is checked out in the fixture's linked worktree"
        );
    });

    // Typed by hand rather than picked from the (disabled) row -- the
    // dialog does not pre-validate the field, so this reaches the same
    // `BranchInUse` refusal the underlying `wtm add` core enforces.
    cx.simulate_input("feature-x");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(2));

    view.read_with(cx, |app, _| {
        let Some(Dialog::Create(state)) = &app.dialog else {
            panic!("dialog should still be open, showing the failure");
        };
        let CreatePhase::Progress(progress) = &state.phase else {
            panic!("expected the progress phase");
        };
        let outcome = progress
            .outcome
            .as_ref()
            .expect("create should have finished (with an error)");
        assert!(
            outcome.is_err(),
            "a branch already checked out elsewhere must be refused"
        );
    });

    assert_eq!(
        view.read_with(cx, |app, _| app.rows.len()),
        before_rows,
        "nothing must be added to the row set"
    );
    assert_eq!(
        fx.list_worktrees().len(),
        before_worktrees,
        "nothing must be created on disk"
    );
}

// ---------------------------------------------------------------------
// 4. Base ref picker
// ---------------------------------------------------------------------

#[gpui::test]
fn base_ref_picker_lists_filters_and_chosen_base_is_honored(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let develop_sha = fx.git(fx.root(), &["rev-parse", "develop"]);
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    cx.simulate_keystrokes("cmd-n");
    cx.simulate_input("from-develop");

    // Focusing the Base field is what opens its picker in the real app —
    // see `CreateState::new`'s `on_focus` subscription — so this is the
    // same trigger a click or Tab would use, not a shortcut around it.
    view.update_in(cx, |app, window, cx| {
        let Some(Dialog::Create(state)) = &app.dialog else {
            panic!("dialog must be open");
        };
        let handle = state.base_input.focus_handle(cx);
        window.focus(&handle);
    });
    cx.run_until_parked();

    view.read_with(cx, |app, _cx| {
        let Some(Dialog::Create(state)) = &app.dialog else {
            panic!("dialog must be open");
        };
        assert!(
            state.base_picker_open,
            "focusing the base field opens the picker"
        );
        assert!(
            state
                .base_refs
                .iter()
                .any(|r| matches!(r.kind, data::RefKind::Remote { .. })),
            "the picker must list the fixture's remote-tracking ref: {:?}",
            state.base_refs.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        assert!(state.base_refs.iter().any(|r| r.name == "develop"));

        let narrowed = dialogs::filter_refs(&state.base_refs, "develop");
        assert_eq!(narrowed.len(), 1, "typing narrows the picker to the match");
        assert_eq!(narrowed[0].name, "develop");
    });

    cx.simulate_input("develop");
    cx.simulate_keystrokes("enter"); // picker open -> picks the sole highlighted match

    view.read_with(cx, |app, cx| {
        let Some(Dialog::Create(state)) = &app.dialog else {
            panic!("dialog must be open");
        };
        assert!(
            !state.base_picker_open,
            "choosing an entry closes the picker"
        );
        assert_eq!(
            state.base_input.read(cx).value(),
            "develop",
            "choosing an entry sets the base"
        );
    });

    cx.simulate_keystrokes("enter"); // picker now closed -> submits the form
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(2));

    view.read_with(cx, |app, _| {
        let Some(Dialog::Create(state)) = &app.dialog else {
            panic!("dialog must be open");
        };
        let CreatePhase::Progress(progress) = &state.phase else {
            panic!("expected the progress phase");
        };
        progress
            .outcome
            .as_ref()
            .expect("create should have finished")
            .as_ref()
            .expect("create with an explicit base must succeed");
    });

    let new_path = fx.worktree_path("from-develop");
    assert!(new_path.is_dir());
    // `git worktree add -b <new> <base>` points the new branch directly at
    // `<base>`'s own commit -- it does not create a new commit on top, so
    // the new worktree's HEAD (not HEAD^, its *parent*) is what must match
    // the chosen base's tip.
    let tip = fx.git(&new_path, &["rev-parse", "HEAD"]);
    assert_eq!(
        tip, develop_sha,
        "the worktree must be branched from the chosen base, not the default"
    );
}

// ---------------------------------------------------------------------
// 5. Remove
// ---------------------------------------------------------------------

#[gpui::test]
fn remove_deletes_clean_worktree_from_disk(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let clean_path = fx.add_worktree("clean-feature");
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    let info = view.read_with(cx, |app, _| {
        app.rows
            .iter()
            .find(|r| r.display_name() == "clean-feature")
            .cloned()
            .unwrap()
    });

    view.update_in(cx, |app, _window, cx| app.open_remove_dialog_for(info, cx));
    view.read_with(cx, |app, _| {
        let Some(Dialog::Remove(state)) = &app.dialog else {
            panic!("remove dialog must be open");
        };
        assert!(
            state.can_confirm(),
            "a clean, non-main worktree may be removed without force"
        );
    });

    view.update_in(cx, |app, _window, cx| app.confirm_remove_dialog(cx));
    cx.run_until_parked();

    view.read_with(cx, |app, _| {
        assert!(app.dialog.is_none(), "the dialog closes on success");
    });
    assert!(!clean_path.exists(), "the directory must be gone from disk");
    assert!(
        !fx.list_worktrees()
            .iter()
            .any(|w| w.display_name() == "clean-feature"),
        "git's own worktree registry must no longer list it"
    );
    view.read_with(cx, |app, _| {
        assert!(
            !app.rows.iter().any(|r| r.display_name() == "clean-feature"),
            "the app's own row set must have dropped it too"
        );
    });
}

#[gpui::test]
fn remove_refuses_main_worktree(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    let main_info = view.read_with(cx, |app, _| {
        app.rows.iter().find(|r| r.is_main).cloned().unwrap()
    });

    view.update_in(cx, |app, _window, cx| {
        app.open_remove_dialog_for(main_info, cx)
    });
    view.read_with(cx, |app, _| {
        let Some(Dialog::Remove(state)) = &app.dialog else {
            panic!("remove dialog must be open");
        };
        assert!(
            !state.can_confirm(),
            "the main worktree may never be confirmed for removal"
        );
    });

    view.update_in(cx, |app, _window, cx| app.confirm_remove_dialog(cx));
    cx.run_until_parked();

    view.read_with(cx, |app, _| {
        assert!(
            matches!(app.dialog, Some(Dialog::Remove(_))),
            "the guarded confirm must be a no-op, not close the dialog"
        );
    });
    assert!(
        fx.root().join(".git").exists(),
        "the main worktree must be untouched on disk"
    );
}

#[gpui::test]
fn remove_dirty_worktree_requires_force(cx: &mut TestAppContext) {
    let fx = Fixture::new(); // feature-x is dirty by construction
    let feature_path = fx.worktree_path("feature-x");
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    let info = view.read_with(cx, |app, _| {
        app.rows
            .iter()
            .find(|r| r.display_name() == "feature-x")
            .cloned()
            .unwrap()
    });
    assert!(info.status.as_ref().unwrap().dirty);

    view.update_in(cx, |app, _window, cx| app.open_remove_dialog_for(info, cx));
    view.read_with(cx, |app, _| {
        let Some(Dialog::Remove(state)) = &app.dialog else {
            panic!("remove dialog must be open");
        };
        assert!(
            !state.can_confirm(),
            "a dirty worktree must not confirm without force"
        );
    });

    // Confirming without force must be a no-op.
    view.update_in(cx, |app, _window, cx| app.confirm_remove_dialog(cx));
    cx.run_until_parked();
    assert!(feature_path.is_dir(), "must survive the guarded confirm");

    view.update_in(cx, |app, _window, cx| app.toggle_remove_force(cx));
    view.read_with(cx, |app, _| {
        let Some(Dialog::Remove(state)) = &app.dialog else {
            panic!("remove dialog must be open");
        };
        assert!(state.force);
        assert!(state.can_confirm(), "force unlocks a dirty worktree");
    });

    view.update_in(cx, |app, _window, cx| app.confirm_remove_dialog(cx));
    cx.run_until_parked();

    view.read_with(cx, |app, _| assert!(app.dialog.is_none()));
    assert!(
        !feature_path.exists(),
        "--force must remove the dirty worktree"
    );
}

// ---------------------------------------------------------------------
// 6. Prune
// ---------------------------------------------------------------------

#[gpui::test]
fn prune_computes_candidates_and_reports_removed_and_skipped_honestly(cx: &mut TestAppContext) {
    let fx = Fixture::new();

    let missing_path = fx.add_worktree("missing-branch");
    std::fs::remove_dir_all(&missing_path).unwrap();

    let merged_dirty_path = fx.add_worktree("merged-dirty"); // same tip as main -> merged
    fx.write_untracked(&merged_dirty_path, "wip.txt", "not committed\n");

    let merged_clean_path = fx.add_worktree("merged-clean");

    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    cx.simulate_keystrokes("cmd-shift-p");
    view.read_with(cx, |app, _| {
        let Some(Dialog::Prune(state)) = &app.dialog else {
            panic!("prune dialog must be open");
        };
        let names: Vec<String> = state
            .candidates
            .iter()
            .map(|c| c.info.display_name().to_string())
            .collect();
        assert_eq!(
            names,
            vec!["missing-branch"],
            "only missing/prunable shows before the merged/gone toggles"
        );
    });

    view.update_in(cx, |app, _window, cx| app.toggle_prune_merged(cx));
    view.read_with(cx, |app, _| {
        let Some(Dialog::Prune(state)) = &app.dialog else {
            panic!("prune dialog must be open");
        };
        let mut names: Vec<String> = state
            .candidates
            .iter()
            .map(|c| c.info.display_name().to_string())
            .collect();
        names.sort();
        // `feature-x` is branched straight from `main`'s tip with no
        // commits of its own (see `Fixture::new`), so its tip *is* an
        // ancestor of `main` -- it is a real, correct merged candidate too,
        // exactly like `merged-clean`/`merged-dirty`. Verified independently
        // against the real `wtm` CLI: `wtm prune --merged --dry-run` against
        // the equivalent fixture reports `feature-x ... [merged]`.
        assert_eq!(
            names,
            vec![
                "feature-x",
                "merged-clean",
                "merged-dirty",
                "missing-branch"
            ],
            "the merged toggle adds every merged candidate, including feature-x"
        );
    });

    view.update_in(cx, |app, _window, cx| app.confirm_prune_dialog(cx));
    // See `run_until`'s doc comment: `finish_prune_dialog` sets `status`
    // and then immediately reloads, whose own with-status pass would clear
    // a purely informational status again if this let it fully settle
    // first.
    run_until(cx, &view, |app: &WtmApp| app.status.is_some());

    view.read_with(cx, |app, _| {
        assert!(app.dialog.is_none());
        let status = app
            .status
            .as_ref()
            .expect("a status message must report the outcome");
        // Removed: `missing-branch` (always force-removed) and
        // `merged-clean`. Skipped (dirty, no force): `feature-x` and
        // `merged-dirty`.
        assert!(status.text.contains("pruned 2"), "{}", status.text);
        assert!(
            status.text.contains("feature-x") && status.text.contains("merged-dirty"),
            "both dirty candidates must be named as skipped: {}",
            status.text
        );
        assert!(
            !status.error,
            "a partial success (two honest dirty skips) is not itself a failure"
        );
    });
    cx.run_until_parked();

    assert!(
        !merged_clean_path.exists(),
        "the clean merged candidate is removed"
    );
    assert!(
        !fx.branch_exists("merged-clean"),
        "a merged candidate's branch is deleted too"
    );
    assert!(
        merged_dirty_path.is_dir(),
        "the dirty candidate must survive an unforced prune"
    );
    assert!(fx.branch_exists("merged-dirty"));
    assert!(
        fx.worktree_path("feature-x").is_dir(),
        "feature-x is also a merged-but-dirty candidate and must survive too"
    );
    assert!(fx.branch_exists("feature-x"));
    assert!(
        !fx.list_worktrees()
            .iter()
            .any(|w| w.display_name() == "missing-branch"),
        "the stale registry entry for the missing worktree must be cleaned up"
    );
}

// ---------------------------------------------------------------------
// 7. Multi-select
// ---------------------------------------------------------------------

#[gpui::test]
fn multi_select_toggle_and_shift_range(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    fx.add_worktree("clean-a");
    fx.add_worktree("clean-b");
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    let main_ix = view.read_with(cx, |app, _| {
        app.rows.iter().position(|r| r.is_main).unwrap()
    });
    let a_ix = view.read_with(cx, |app, _| {
        app.rows
            .iter()
            .position(|r| r.display_name() == "clean-a")
            .unwrap()
    });
    let b_ix = view.read_with(cx, |app, _| {
        app.rows
            .iter()
            .position(|r| r.display_name() == "clean-b")
            .unwrap()
    });

    // ⌘-click twice toggles two different rows on; a third ⌘-click on the
    // first toggles it back off, exactly what a checkbox click or an
    // actual ⌘-click's `on_click` handler in `chrome.rs` invokes.
    view.update_in(cx, |app, _window, cx| app.toggle_row_selection(main_ix, cx));
    view.update_in(cx, |app, _window, cx| app.toggle_row_selection(a_ix, cx));
    view.read_with(cx, |app, _| {
        assert_eq!(app.multi_selected, BTreeSet::from([main_ix, a_ix]));
        assert_eq!(
            app.selected,
            Some(a_ix),
            "the last-toggled row becomes the anchor"
        );
    });
    view.update_in(cx, |app, _window, cx| app.toggle_row_selection(main_ix, cx));
    view.read_with(cx, |app, _| {
        assert!(
            app.multi_selected.is_empty(),
            "a set that shrinks to one row collapses back to plain selection"
        );
        assert_eq!(app.selected, Some(a_ix));
    });

    // Shift-click: select every visible row between the anchor and the target.
    view.update_in(cx, |app, _window, cx| app.select(main_ix, cx));
    view.update_in(cx, |app, _window, cx| app.extend_selection_range(b_ix, cx));
    view.read_with(cx, |app, cx| {
        let visible = app.visible_row_indices(cx);
        let a_pos = visible.iter().position(|&r| r == main_ix).unwrap();
        let b_pos = visible.iter().position(|&r| r == b_ix).unwrap();
        let (lo, hi) = (a_pos.min(b_pos), a_pos.max(b_pos));
        let expected: BTreeSet<usize> = visible[lo..=hi].iter().copied().collect();
        assert_eq!(app.multi_selected, expected);
        assert!(
            app.multi_selected.len() >= 2,
            "a real range must cover more than one row"
        );
    });
}

#[gpui::test]
fn bulk_remove_applies_to_selection_and_protects_main(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let a_path = fx.add_worktree("clean-a");
    let b_path = fx.add_worktree("clean-b");
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    let a_ix = view.read_with(cx, |app, _| {
        app.rows
            .iter()
            .position(|r| r.display_name() == "clean-a")
            .unwrap()
    });
    let b_ix = view.read_with(cx, |app, _| {
        app.rows
            .iter()
            .position(|r| r.display_name() == "clean-b")
            .unwrap()
    });

    view.update_in(cx, |app, _window, cx| app.toggle_row_selection(a_ix, cx));
    view.update_in(cx, |app, _window, cx| app.toggle_row_selection(b_ix, cx));

    // ⌘⌫ with a multi-selection opens the bulk-remove confirmation instead
    // of the single-target Remove dialog — see `on_remove_selected`.
    cx.simulate_keystrokes("cmd-backspace");
    view.read_with(cx, |app, _| {
        let state = app
            .bulk_remove
            .as_ref()
            .expect("bulk remove confirmation must be open");
        let names: BTreeSet<String> = state
            .candidates
            .iter()
            .map(|c| c.info.display_name().to_string())
            .collect();
        assert_eq!(
            names,
            BTreeSet::from(["clean-a".to_string(), "clean-b".to_string()])
        );
    });

    view.update_in(cx, |app, _window, cx| app.confirm_bulk_remove(cx));
    // See `run_until`'s doc comment: `finish_bulk_remove` sets `status` and
    // then immediately reloads, whose own with-status pass would otherwise
    // clear this purely informational status before a full
    // `run_until_parked` returned.
    run_until(cx, &view, |app: &WtmApp| app.status.is_some());

    view.read_with(cx, |app, _| {
        assert!(app.bulk_remove.is_none());
        assert!(app.multi_selected.is_empty());
        let status = app.status.as_ref().unwrap();
        assert!(status.text.contains("removed 2"), "{}", status.text);
    });
    cx.run_until_parked();
    assert!(!a_path.exists());
    assert!(!b_path.exists());

    // A selection that mixes in the main worktree still only ever offers
    // the real candidate -- main is excluded outright, never even shown
    // as something to skip.
    let main_ix = view.read_with(cx, |app, _| {
        app.rows.iter().position(|r| r.is_main).unwrap()
    });
    let feature_ix = view.read_with(cx, |app, _| {
        app.rows
            .iter()
            .position(|r| r.display_name() == "feature-x")
            .unwrap()
    });
    // `select` first (not another `toggle_row_selection`) to force a known
    // baseline: `toggle_row_selection` *toggles* membership, and only two
    // rows are left after the removal above, so whichever one `apply_rows`
    // left as the plain single selection would otherwise make the very
    // next toggle a no-op round trip (or, worse, a removal) instead of the
    // intended "add" -- see `bulk_remove_applies_to_selection_and_protects_main`'s
    // original failure, which hit exactly this.
    view.update_in(cx, |app, _window, cx| app.select(main_ix, cx));
    view.update_in(cx, |app, _window, cx| {
        app.toggle_row_selection(feature_ix, cx)
    });
    cx.simulate_keystrokes("cmd-backspace");
    view.read_with(cx, |app, _| {
        let state = app
            .bulk_remove
            .as_ref()
            .expect("bulk remove confirmation must be open");
        assert_eq!(
            state.candidates.len(),
            1,
            "main is never offered, even when selected"
        );
        assert_eq!(state.candidates[0].info.display_name(), "feature-x");
    });
}

// ---------------------------------------------------------------------
// 8. Filter
// ---------------------------------------------------------------------

#[gpui::test]
fn filter_narrows_rows_and_escape_clears_it(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    fx.add_worktree("other-thing");
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    let total = view.read_with(cx, |app, _| app.rows.len());
    assert_eq!(total, 3, "main, feature-x, other-thing");

    cx.simulate_keystrokes("cmd-f");
    cx.simulate_input("feat");

    view.read_with(cx, |app, cx| {
        let visible = app.visible_row_indices(cx);
        assert!(
            !visible.is_empty() && visible.len() < total,
            "the filter must narrow the row set, not clear or ignore it: {visible:?}"
        );
        for &ix in &visible {
            assert!(
                palette::fuzzy_match("feat", app.rows[ix].display_name()).is_some(),
                "every visible row must actually match the query"
            );
        }
        assert!(
            app.selected.is_some_and(|ix| visible.contains(&ix)),
            "the selection must never point at a hidden row"
        );
    });

    cx.simulate_keystrokes("escape");
    view.read_with(cx, |app, cx| {
        assert_eq!(app.filter_input.read(cx).value(), "");
        assert_eq!(
            app.visible_row_indices(cx).len(),
            total,
            "escape clears the filter"
        );
    });
}

// ---------------------------------------------------------------------
// 9. Command palette
// ---------------------------------------------------------------------

#[gpui::test]
fn command_palette_filters_and_enter_selects_worktree(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    cx.simulate_keystrokes("cmd-k");
    view.read_with(cx, |app, _| {
        assert!(app.palette.is_some(), "cmd-k opens the palette")
    });

    cx.simulate_input("feature-x");
    cx.simulate_keystrokes("enter");

    view.read_with(cx, |app, _| {
        assert!(
            app.palette.is_none(),
            "Enter selects and closes the palette"
        );
        let selected = app.selected.and_then(|ix| app.rows.get(ix));
        assert_eq!(selected.map(|r| r.display_name()), Some("feature-x"));
    });
}

// ---------------------------------------------------------------------
// 10. Detail panel tabs
// ---------------------------------------------------------------------

#[gpui::test]
fn detail_panel_tabs_load_real_files_and_changes(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    let feature_ix = view.read_with(cx, |app, _| {
        app.rows
            .iter()
            .position(|r| r.display_name() == "feature-x")
            .unwrap()
    });
    view.update_in(cx, |app, _window, cx| app.select(feature_ix, cx));
    cx.run_until_parked();

    let feature_path = view.read_with(cx, |app, _| app.rows[feature_ix].path.clone());

    view.read_with(cx, |app, _| {
        let tree = app
            .file_trees
            .get(&feature_path)
            .expect("the root directory must have started loading on selection");
        match tree.dir_state(Path::new("")) {
            Some(file_browser::DirState::Loaded(entries)) => {
                assert!(
                    entries.iter().any(|e| e.name == "scratch.txt"),
                    "{entries:?}"
                );
            }
            other => panic!("expected the root listing to be loaded, got {other:?}"),
        }

        match &app.changes {
            ChangesState::Loaded(diffs) => {
                assert!(diffs.iter().any(|d| d.path == "scratch.txt"));
            }
            ChangesState::Loading => panic!("the Changes tab's diff never finished loading"),
            ChangesState::Error(e) => panic!("the Changes tab failed to load: {e}"),
        }
    });

    assert_eq!(
        view.read_with(cx, |app, _| app.detail_tab),
        DetailTab::Details
    );
    cx.simulate_keystrokes("cmd-2");
    assert_eq!(
        view.read_with(cx, |app, _| app.detail_tab),
        DetailTab::Files
    );
    cx.simulate_keystrokes("cmd-3");
    assert_eq!(
        view.read_with(cx, |app, _| app.detail_tab),
        DetailTab::Changes
    );
    cx.simulate_keystrokes("cmd-1");
    assert_eq!(
        view.read_with(cx, |app, _| app.detail_tab),
        DetailTab::Details
    );

    let visible_before = view.read_with(cx, |app, _| app.detail_panel_visible);
    cx.simulate_keystrokes("cmd-i");
    assert_eq!(
        view.read_with(cx, |app, _| app.detail_panel_visible),
        !visible_before,
        "cmd-i toggles the panel"
    );
    cx.simulate_keystrokes("cmd-i");
    assert_eq!(
        view.read_with(cx, |app, _| app.detail_panel_visible),
        visible_before
    );
}

// ---------------------------------------------------------------------
// 11. Add repository
//
// See this module's doc comment ("What could not be driven headlessly at
// all") for why these call `finish_add_repository` directly rather than
// `on_add_repository` + `cx.simulate_new_path_selection`: gpui 0.2.2's
// `TestPlatform::prompt_for_paths` (the "choose a directory" picker
// `AddRepository` calls) is `unimplemented!()`, unlike `prompt_for_new_path`
// (a *save* dialog), which is the one `simulate_new_path_selection` drives.
// ---------------------------------------------------------------------

#[gpui::test]
fn add_repository_resolves_and_activates_chosen_directory(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let second = fx.sibling_repo("second-repo");
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    view.update_in(cx, |app, _window, cx| {
        app.finish_add_repository(second.clone(), cx)
    });
    cx.run_until_parked();

    view.read_with(cx, |app, _| {
        let active = app.active.as_ref().expect("a repository must be active");
        assert_eq!(active.path(), second.as_path());
        assert!(
            app.repos.iter().any(|r| r.path == second),
            "the sidebar registry must include it"
        );
        assert!(
            app.rows.iter().any(|r| r.is_main),
            "the new repo's worktrees must have loaded"
        );
    });
}

#[gpui::test]
fn add_repository_rejects_non_repository_directory_with_a_message(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let not_a_repo = fx.non_repo_dir("not-a-repo");
    let (view, cx) = open_app(cx, Some(repo.clone()));
    cx.run_until_parked();

    view.update_in(cx, |app, _window, cx| {
        app.finish_add_repository(not_a_repo.clone(), cx)
    });
    cx.run_until_parked();

    view.read_with(cx, |app, _| {
        assert_eq!(
            app.active.as_ref().unwrap().path(),
            repo.path(),
            "the active repository must not change"
        );
        assert!(!app.repos.iter().any(|r| r.path == not_a_repo));
        let status = app
            .status
            .as_ref()
            .expect("a rejection message must be shown");
        assert!(status.error);
        assert!(
            status.text.contains("not a git repository"),
            "{}",
            status.text
        );
    });
}

// ---------------------------------------------------------------------
// 12. Escape layering
// ---------------------------------------------------------------------

#[gpui::test]
fn escape_closes_picker_then_dialog_then_is_a_noop(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    cx.simulate_keystrokes("cmd-n");
    view.update_in(cx, |app, window, cx| {
        let Some(Dialog::Create(state)) = &app.dialog else {
            panic!("dialog must be open");
        };
        let handle = state.base_input.focus_handle(cx);
        window.focus(&handle);
    });
    cx.run_until_parked();
    view.read_with(cx, |app, _| {
        let Some(Dialog::Create(state)) = &app.dialog else {
            panic!("dialog must be open");
        };
        assert!(state.base_picker_open, "the picker must be open first");
    });

    // First Escape: closes the picker only, leaving the dialog (and
    // whatever was typed) alone.
    cx.simulate_keystrokes("escape");
    view.read_with(cx, |app, _| {
        let Some(Dialog::Create(state)) = &app.dialog else {
            panic!("the dialog itself must survive the first Escape");
        };
        assert!(!state.base_picker_open);
    });

    // Second Escape: the picker is already gone, so this one closes the
    // dialog itself.
    cx.simulate_keystrokes("escape");
    view.read_with(cx, |app, _| assert!(app.dialog.is_none()));

    // Third Escape: nothing left to close -- a no-op, never destructive.
    let rows_before = view.read_with(cx, |app, _| app.rows.len());
    cx.simulate_keystrokes("escape");
    view.read_with(cx, |app, _| {
        assert!(app.dialog.is_none());
        assert_eq!(app.rows.len(), rows_before);
    });
}

#[gpui::test]
fn escape_clears_multi_selection_without_closing_anything(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    let main_ix = view.read_with(cx, |app, _| {
        app.rows.iter().position(|r| r.is_main).unwrap()
    });
    let feature_ix = view.read_with(cx, |app, _| {
        app.rows
            .iter()
            .position(|r| r.display_name() == "feature-x")
            .unwrap()
    });
    // `select` then `toggle` (not two toggles) for a deterministic
    // baseline regardless of which row happened to be selected by default
    // -- see the same pattern's comment in
    // `bulk_remove_applies_to_selection_and_protects_main`.
    view.update_in(cx, |app, _window, cx| app.select(main_ix, cx));
    view.update_in(cx, |app, _window, cx| {
        app.toggle_row_selection(feature_ix, cx)
    });
    view.read_with(cx, |app, _| assert_eq!(app.multi_selected.len(), 2));

    cx.simulate_keystrokes("escape");

    view.read_with(cx, |app, _| {
        assert!(
            app.multi_selected.is_empty(),
            "escape collapses a multi-selection instead of leaving it"
        );
        assert!(app.dialog.is_none());
        assert_eq!(app.rows.len(), 2, "nothing must be removed by an Escape");
    });
}

//! Headless integration tests for the whole app, driven through gpui's
//! `test-support` harness (`TestAppContext`/`#[gpui::test]`).
//!
//! Declared as a child of `app` (`app/mod.rs`'s `#[cfg(test)] mod
//! integration_tests;`) rather than a top-level `tests/` crate, so it can
//! see `WtmApp`'s private fields (`rows`, `selected`, `dialog`, …) directly
//! and drive its real `pub(super)`/`pub(crate)` handler methods instead of
//! reconstructing state from rendered output.
//!
//! Opening a repository always starts a real filesystem watcher
//! (`WtmApp::apply_rows` -> `sync_watcher`), whose consumer task blocks on
//! `rx.recv()` forever under `TestDispatcher`'s single-threaded,
//! cooperative model — hanging `run_until_parked`. Every test here calls
//! `disable_watcher_for_tests()` before opening a window.
//!
//! `run_until_parked` drains everything currently ready, including a
//! completed action's own follow-up `reload` — whose with-status pass
//! clears a non-error status message the instant it lands. Asserting on
//! such a status after a full `run_until_parked` races that reload and
//! loses. `run_until` (below) stops the instant a predicate is satisfied
//! instead, before the follow-up reload gets a chance to run.

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
    previous: Option<std::ffi::OsString>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn set(dir: &Path) -> Self {
        let lock = prefs::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var_os("WTM_CONFIG_DIR");
        std::env::set_var("WTM_CONFIG_DIR", dir);
        EnvGuard {
            previous,
            _lock: lock,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("WTM_CONFIG_DIR", value),
            None => std::env::remove_var("WTM_CONFIG_DIR"),
        }
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

/// Run a fixture-setup git command with an explicit author/committer date,
/// otherwise identical to `git` above. Used only where a test needs a
/// *known, controlled* commit time (`worktree_activity`/`Recent`-sort
/// tests) — an ordinary commit's real wall-clock time is fine everywhere
/// else, but two commits made microseconds apart in a fast test run can
/// land in the same second, which would make an ordering assertion flaky.
fn git_with_date(dir: &Path, args: &[&str], epoch_secs: i64) -> String {
    let date = format!("{epoch_secs} +0000");
    let out = StdCommand::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "wtm-gui test")
        .env("GIT_AUTHOR_EMAIL", "wtm-gui@example.invalid")
        .env("GIT_COMMITTER_NAME", "wtm-gui test")
        .env("GIT_COMMITTER_EMAIL", "wtm-gui@example.invalid")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
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
    fn advance_branch(&self, branch: &str, filename: &str, contents: &str) {
        let scratch = self.base.join(format!("scratch-{branch}"));
        git(
            &self.root,
            &["worktree", "add", scratch.to_str().unwrap(), branch],
        );
        std::fs::write(scratch.join(filename), contents).unwrap();
        git(&scratch, &["add", "."]);
        git(&scratch, &["commit", "-m", &format!("advance {branch}")]);
        git(
            &self.root,
            &["worktree", "remove", "--force", scratch.to_str().unwrap()],
        );
    }

    fn write_untracked(&self, dir: &Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).expect("write untracked file");
    }

    /// Add a linked worktree on a new branch whose one commit (beyond the
    /// shared tip every fresh worktree otherwise starts at) is stamped at
    /// `epoch_secs` via [`git_with_date`] — for `Recent`-sort tests, which
    /// need a *known* ordering of commit times, not just "whichever commit
    /// happened to run first." Clean until the caller writes something
    /// else into it.
    fn add_worktree_with_commit_at(&self, branch: &str, epoch_secs: i64) -> PathBuf {
        let path = self.worktree_path(branch);
        git(
            &self.root,
            &["worktree", "add", path.to_str().unwrap(), "-b", branch],
        );
        std::fs::write(path.join(format!("{branch}.txt")), "content\n").unwrap();
        git(&path, &["add", "."]);
        git_with_date(
            &path,
            &["commit", "-m", &format!("advance {branch}")],
            epoch_secs,
        );
        path
    }

    fn branch_exists(&self, name: &str) -> bool {
        !git(&self.root, &["branch", "--list", name]).is_empty()
    }

    /// The library's own worktree listing (not the app's `rows`) — for
    /// asserting against the real repository, independent of anything the
    /// app might have gotten wrong.
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
    // Tick one task at a time so the predicate is checked between tasks
    // (a full drain would let the follow-up reload clear the status). When
    // the dispatcher parks it may only be waiting on a timer — the
    // drain-stream loop polls its channel every 16ms under `cfg(test)` — so
    // move the virtual clock to the next timer instead of failing; a state
    // that never arrives still fails with a message, not a hang.
    let executor = cx.executor().clone();
    let dispatcher = executor.dispatcher.as_test().unwrap();
    let mut nudges = 0;
    while !view.read_with(cx, |v, _| predicate(v)) {
        if !executor.tick() {
            nudges += 1;
            assert!(
                nudges < 100_000 && dispatcher.advance_clock_to_next_delayed(),
                "dispatcher fully parked before the expected state was ever reached"
            );
        }
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
    let develop_sha = git(fx.root(), &["rev-parse", "develop"]);
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
    let tip = git(&new_path, &["rev-parse", "HEAD"]);
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

    view.update_in(cx, |app, window, cx| {
        app.open_remove_dialog_for(info, window, cx)
    });
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
        "git's own worktree registry must not list it"
    );
    view.read_with(cx, |app, _| {
        assert!(
            !app.rows.iter().any(|r| r.display_name() == "clean-feature"),
            "the app's own row set must have dropped it too"
        );
    });
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

    view.update_in(cx, |app, window, cx| {
        app.open_remove_dialog_for(info, window, cx)
    });
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
    view.read_with(cx, |app, _| {
        assert!(
            matches!(app.dialog, Some(Dialog::Remove(_))),
            "the guarded confirm must be a no-op, not close the dialog"
        );
    });

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

/// A watcher notification that lands while a prune is running in the
/// background must only mark the repository stale, not start a reload
/// itself — `on_watcher_change` guards on `prune_in_flight` exactly like it
/// guards on `loading`. The prune's own completion (`report_prune`) must be
/// the one reload that actually runs.
#[gpui::test]
fn watcher_changes_during_a_prune_only_mark_stale_and_reload_once_at_the_end(
    cx: &mut TestAppContext,
) {
    let fx = Fixture::new();
    let merged_a = fx.add_worktree("merged-a");
    let merged_b = fx.add_worktree("merged-b");

    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    cx.simulate_keystrokes("cmd-shift-p");
    view.update_in(cx, |app, _window, cx| app.toggle_prune_merged(cx));
    view.read_with(cx, |app, _| {
        let Some(Dialog::Prune(state)) = &app.dialog else {
            panic!("prune dialog must be open");
        };
        assert!(
            !state.candidates.is_empty(),
            "the merged toggle must surface merged-a/merged-b as candidates"
        );
    });

    view.update_in(cx, |app, _window, cx| app.confirm_prune_dialog(cx));
    // Record the generation right after confirming, before letting the
    // dispatcher drive the background prune forward at all.
    let gen0 = view.read_with(cx, |app, _| app.generation);

    view.update_in(cx, |app, _window, cx| {
        app.on_watcher_change(cx);
        app.on_watcher_change(cx);
        app.on_watcher_change(cx);
    });
    view.read_with(cx, |app, _| {
        assert!(app.prune_in_flight, "the prune must still be running");
        assert!(
            app.repository_stale,
            "each watcher event must still record the stale bit"
        );
        assert_eq!(
            app.generation, gen0,
            "no reload may start while a prune is in flight"
        );
    });

    // Now let the background prune actually finish. Like the run-command
    // dialog's own streaming test, the drain loop waits for channel events
    // on the background executor via a polling timer, so the cooperative
    // dispatcher needs its clock advanced to make that timer fire
    // deterministically instead of parking forever.
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(2));

    view.read_with(cx, |app, _| {
        assert!(!app.prune_in_flight, "the prune must have finished");
        assert!(app.dialog.is_none());
        assert!(
            !app.repository_stale,
            "the completion's own reload must consume the stale bit"
        );
        assert!(
            app.generation > gen0,
            "exactly the prune's own completion reload must have run"
        );
    });
    assert!(!merged_a.is_dir(), "merged-a must have been pruned");
    assert!(!merged_b.is_dir(), "merged-b must have been pruned");
    assert!(
        fx.worktree_path("feature-x").is_dir(),
        "feature-x is merged but dirty, so an unforced prune must leave it"
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
    view.read_with(cx, |app, _| {
        // Default Name sort with main pinned first puts the fixture's rows
        // in order main, clean-a, clean-b, feature-x, so a range from main
        // to clean-b covers exactly these three.
        assert_eq!(app.multi_selected, BTreeSet::from([main_ix, a_ix, b_ix]));
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
    });
}

/// `Registry::entries()` sorts most-recently-opened first, and selecting a
/// repository calls `registry::remember`, which bumps `last_opened` — so if
/// the sidebar rendered that order directly, the repo you just clicked
/// would jump to the top and the whole list would reshuffle under your
/// cursor. `app.repos` must stay in its own stable (alphabetical) order
/// regardless of which repo was most recently selected.
#[gpui::test]
fn selecting_a_repo_does_not_reorder_the_sidebar(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let alpha = fx.sibling_repo("alpha-repo");
    let zulu = fx.sibling_repo("zulu-repo");
    // `fx.root()` is named "repo" — alphabetically between the two above.
    let (view, cx) = open_app(cx, Some(fx.open()));
    cx.run_until_parked();

    // Add both siblings; each `finish_add_repository` call activates (and
    // so `remember`s) the repo it adds, leaving `zulu-repo` the most
    // recently opened of the three.
    view.update_in(cx, |app, _window, cx| {
        app.finish_add_repository(alpha.clone(), cx)
    });
    cx.run_until_parked();
    view.update_in(cx, |app, _window, cx| {
        app.finish_add_repository(zulu.clone(), cx)
    });
    cx.run_until_parked();

    let order_before: Vec<PathBuf> = view.read_with(cx, |app, _| {
        app.repos.iter().map(|r| r.path.clone()).collect()
    });
    assert_eq!(
        order_before,
        vec![alpha.clone(), fx.root().to_path_buf(), zulu.clone()],
        "the sidebar must start alphabetical by name, not most-recently-opened"
    );

    // Select the repo furthest from the top of the most-recently-opened
    // order (the fixture's own root, opened first of the three) — this is
    // exactly the click the bug report described.
    view.update_in(cx, |app, _window, cx| {
        app.select_repo(fx.root().to_path_buf(), cx)
    });
    cx.run_until_parked();

    let order_after: Vec<PathBuf> = view.read_with(cx, |app, _| {
        app.repos.iter().map(|r| r.path.clone()).collect()
    });
    assert_eq!(
        order_before, order_after,
        "selecting a repo must not reorder the sidebar"
    );
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

// ---------------------------------------------------------------------
// 18. Sorting
// ---------------------------------------------------------------------

/// Read every row's display name, in listing order — the shape every
/// sort-order assertion below checks.
fn row_names(app: &WtmApp) -> Vec<String> {
    app.rows
        .iter()
        .map(|r| r.display_name().to_string())
        .collect()
}

#[gpui::test]
fn sort_modes_order_rows_correctly_with_main_always_pinned_first(cx: &mut TestAppContext) {
    let fx = Fixture::new(); // main (clean) + feature-x (dirty, same tip/time as main)

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    // `clean-one` shares main's tip (no advance) -- same commit, same time,
    // clean. `old`/`newest` get their own commits stamped far enough from
    // "now" (and from each other) that ordinary test-run jitter can never
    // put them out of the intended order.
    fx.add_worktree("clean-one");
    fx.add_worktree_with_commit_at("old", now - 50_000);
    fx.add_worktree_with_commit_at("newest", now + 50_000);

    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    view.read_with(cx, |app, _| {
        assert_eq!(
            app.activity.len(),
            5, // main, feature-x, clean-one, old, newest
            "every row's HEAD commit time must have loaded in the background"
        );
        assert_eq!(app.sort_mode, SortMode::Name);
    });

    // Exact per-mode ordering is `worktree_list::sort_rows`'s own unit
    // tests' job; this only needs to know a mode switch actually re-sorts
    // the live list, with main still pinned first.
    let name_order = view.read_with(cx, |app, _| row_names(app));
    view.update_in(cx, |app, _window, cx| {
        app.set_sort_mode(SortMode::Recent, cx)
    });
    view.read_with(cx, |app, _| {
        assert_eq!(app.sort_mode, SortMode::Recent);
        assert_ne!(
            row_names(app),
            name_order,
            "switching sort modes re-sorts the list"
        );
        assert_eq!(row_names(app)[0], "main", "main stays pinned first");
    });
}

#[gpui::test]
fn selection_survives_a_sort_mode_change_by_path_not_index(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    fx.add_worktree_with_commit_at("old", now - 50_000);
    fx.add_worktree_with_commit_at("newest", now + 50_000);
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    // Rows are (Name mode, the default): main, feature-x, newest, old --
    // "newest" sorts third alphabetically among the three non-main rows.
    // Under Recent mode it sorts second (right after main, being the most
    // recent commit of all) -- a different index, which is exactly the
    // case this test needs.
    let (old_ix, old_path) = view.read_with(cx, |app, _| {
        let ix = app
            .rows
            .iter()
            .position(|r| r.display_name() == "old")
            .unwrap();
        (ix, app.rows[ix].path.clone())
    });
    let (newest_ix, newest_path) = view.read_with(cx, |app, _| {
        let ix = app
            .rows
            .iter()
            .position(|r| r.display_name() == "newest")
            .unwrap();
        (ix, app.rows[ix].path.clone())
    });

    // `select` then `toggle` for a deterministic multi-selection baseline
    // regardless of which row happened to be selected by default.
    view.update_in(cx, |app, _window, cx| app.select(old_ix, cx));
    view.update_in(cx, |app, _window, cx| {
        app.toggle_row_selection(newest_ix, cx)
    });
    view.read_with(cx, |app, _| {
        assert_eq!(app.multi_selected.len(), 2);
        assert_eq!(
            app.selected,
            Some(newest_ix),
            "the last-toggled row becomes the anchor"
        );
    });

    // Re-sort to Recent: "newest" moves toward the front of the list (right
    // behind the pinned main worktree), so indices necessarily change. Both
    // the anchor selection and the multi-selection must survive by path,
    // not by index.
    view.update_in(cx, |app, _window, cx| {
        app.set_sort_mode(SortMode::Recent, cx)
    });

    view.read_with(cx, |app, _| {
        let new_ix = app
            .selected
            .expect("still something selected after the re-sort");
        assert_ne!(
            new_ix, newest_ix,
            "the index changing is the whole point of this test"
        );
        assert_eq!(
            app.rows[new_ix].path, newest_path,
            "the SAME worktree, by path, must still be selected"
        );

        let selected_paths: BTreeSet<PathBuf> = app
            .multi_selected
            .iter()
            .map(|&ix| app.rows[ix].path.clone())
            .collect();
        assert_eq!(
            selected_paths,
            BTreeSet::from([old_path.clone(), newest_path.clone()]),
            "both originally-selected worktrees must still be selected, by path, \
             even though the re-sort moved them to different indices"
        );
    });
}

/// A background `RepoWatcher` tick (modeled here by calling
/// `WtmApp::reload` directly — the same path the watcher's
/// `on_watcher_change` and the manual ⌘R both take) that lands while a
/// *different* worktree's status changed can reorder rows under
/// `SortMode::Status` without the user touching the selected row at all.
/// `apply_rows` must track the selection by path, not by raw index, so a
/// reorder like this can never silently re-point `selected` at whatever
/// row happened to shift into that same slot.
#[gpui::test]
fn selection_survives_a_reload_that_reorders_rows_by_path_not_index(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    fx.add_worktree("clean-one");
    let other = fx.add_worktree("other");
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    view.update_in(cx, |app, _window, cx| {
        app.set_sort_mode(SortMode::Status, cx)
    });
    cx.run_until_parked();

    // Status mode: main, feature-x (dirty, from the fixture's own setup),
    // then the clean rows alphabetically — clean-one, other.
    let (clean_ix_before, clean_path) = view.read_with(cx, |app, _| {
        let ix = app
            .rows
            .iter()
            .position(|r| r.display_name() == "clean-one")
            .unwrap();
        (ix, app.rows[ix].path.clone())
    });
    view.update_in(cx, |app, _window, cx| app.select(clean_ix_before, cx));

    // Dirty a DIFFERENT worktree that currently sorts *after* clean-one —
    // under Status mode it jumps ahead of clean-one the moment it's dirty,
    // so clean-one's index necessarily shifts once the list re-sorts.
    fx.write_untracked(&other, "scratch.txt", "uncommitted\n");

    view.update_in(cx, |app, _window, cx| app.reload(cx));
    cx.run_until_parked();

    view.read_with(cx, |app, _| {
        let new_ix = app
            .selected
            .expect("still something selected after the reload");
        assert_ne!(
            new_ix, clean_ix_before,
            "the index changing is the whole point of this test"
        );
        assert_eq!(
            app.rows[new_ix].path, clean_path,
            "the SAME worktree, by path, must still be selected — not whichever \
             row shifted into its old slot"
        );
    });
}

/// Filesystem notifications must not launch a full status walk while the
/// window is inactive. Multiple notifications are represented by one stale
/// bit and produce exactly one refresh when the window becomes active again.
#[gpui::test]
fn watcher_changes_are_coalesced_while_window_is_inactive(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    let generation_before = view.read_with(cx, |app, _| {
        assert!(app.window_active);
        app.generation
    });
    cx.deactivate_window();

    view.update_in(cx, |app, _window, cx| {
        app.on_watcher_change(cx);
        app.on_watcher_change(cx);
    });
    view.read_with(cx, |app, _| {
        assert!(!app.window_active);
        assert!(app.repository_stale);
        assert_eq!(app.generation, generation_before);
    });

    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    view.read_with(cx, |app, _| {
        assert!(app.window_active);
        assert!(!app.repository_stale);
        assert_eq!(
            app.generation,
            generation_before + 1,
            "activation must issue one coalesced refresh"
        );
    });
}

/// An active watcher event that arrives during a reload must not be dropped:
/// it schedules one follow-up generation after the in-flight status pass.
#[gpui::test]
fn watcher_change_during_reload_gets_one_follow_up_refresh(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    let generation_before = view.read_with(cx, |app, _| {
        assert!(app.window_active);
        app.generation
    });
    view.update_in(cx, |app, _window, cx| {
        app.reload(cx);
        app.on_watcher_change(cx);
    });
    view.read_with(cx, |app, _| {
        assert!(app.loading);
        assert!(app.repository_stale);
        assert_eq!(app.generation, generation_before + 1);
    });

    cx.run_until_parked();
    view.read_with(cx, |app, _| {
        assert!(!app.loading);
        assert!(!app.repository_stale);
        assert_eq!(
            app.generation,
            generation_before + 2,
            "the in-flight event must cause one follow-up refresh"
        );
    });
}

/// If the window is deactivated while a reload is in flight, its completion
/// must not start background work. The stale event waits for activation.
#[gpui::test]
fn watcher_change_during_reload_waits_for_reactivation(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    let generation_before = view.read_with(cx, |app, _| app.generation);
    view.update_in(cx, |app, _window, cx| app.reload(cx));
    cx.deactivate_window();
    view.update_in(cx, |app, _window, cx| app.on_watcher_change(cx));

    cx.run_until_parked();
    view.read_with(cx, |app, _| {
        assert!(!app.window_active);
        assert!(app.repository_stale);
        assert_eq!(
            app.generation,
            generation_before + 1,
            "an inactive completion must not launch a follow-up refresh"
        );
    });

    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    view.read_with(cx, |app, _| {
        assert!(app.window_active);
        assert!(!app.repository_stale);
        assert_eq!(app.generation, generation_before + 2);
    });
}

// ---------------------------------------------------------------------
// 19. Fetch
// ---------------------------------------------------------------------

#[gpui::test]
fn fetch_keybinding_dispatches_and_reports_failure_offline(cx: &mut TestAppContext) {
    // No test fixture in this file ever runs `git remote add` -- every
    // repository `Fixture` builds has zero configured remotes. That makes
    // `data::fetch`'s `default_remote_name` fail *before* it ever
    // constructs a `git fetch` command (see `data.rs`), so dispatching the
    // real ⌘⇧F binding here exercises the real production path end to end
    // — action dispatch, the background spawn, `apply_fetch_result` —
    // without the test ever touching the network, deterministically.
    let fx = Fixture::new();
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    view.read_with(cx, |app, _| {
        assert!(!app.fetching);
        assert!(app.status.is_none());
    });

    cx.simulate_keystrokes("cmd-shift-f");

    view.read_with(cx, |app, _| {
        assert!(!app.fetching, "the guard clears once the fetch settles");
        let status = app
            .status
            .as_ref()
            .expect("⌘⇧F must report an outcome in the status line");
        assert!(status.error, "no configured remote is a real failure");
        assert!(status.text.contains("fetch failed"), "{}", status.text);
    });
}

#[gpui::test]
fn fetch_in_flight_guard_blocks_a_second_concurrent_trigger(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    // Call the handler directly, twice, back to back, with nothing
    // draining the background executor in between. `cx.simulate_keystrokes`
    // always runs the executor to a full park before returning (see
    // `TestAppContext::simulate_keystrokes`), which would let the first
    // (offline, fast-failing) fetch finish before a second trigger could
    // ever observe it in flight. `Entity::update_in` runs only the
    // synchronous body of the closure, so this reliably captures the
    // "first fetch's background task hasn't run yet" window a second
    // trigger during a real, slow `git fetch` would land in — no timing
    // race, and (see the previous test's doc comment) no network involved
    // either way.
    view.update_in(cx, |app, window, cx| {
        app.on_fetch_remote(&FetchRemote, window, cx)
    });
    view.read_with(cx, |app, _| {
        assert!(
            app.fetching,
            "the first trigger must flip the guard synchronously, before its background task runs"
        );
    });

    view.update_in(cx, |app, window, cx| {
        app.on_fetch_remote(&FetchRemote, window, cx)
    });
    view.read_with(cx, |app, _| {
        assert!(
            app.fetching,
            "still in flight -- a second trigger must be a no-op, not start a second fetch"
        );
    });
}

// ---------------------------------------------------------------------
// Run command
// ---------------------------------------------------------------------

#[gpui::test]
fn run_command_that_succeeds_reaches_finished_state_with_expected_output(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    cx.simulate_keystrokes("cmd-e");
    view.read_with(cx, |app, _| {
        assert!(
            matches!(
                app.run_command.as_ref().map(|s| &s.phase),
                Some(run_panel::RunPhase::Form)
            ),
            "cmd-e opens the Run Command dialog on the selected worktree"
        );
    });

    cx.simulate_input("echo hello");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    // The drain loop waits for streamed channel events on the background
    // executor. Advance the cooperative dispatcher so the subprocess and its
    // UI batches finish deterministically.
    cx.executor().advance_clock(Duration::from_secs(2));

    view.read_with(cx, |app, _| {
        let state = app
            .run_command
            .as_ref()
            .expect("dialog should still be open, showing the running phase");
        let run_panel::RunPhase::Running(progress) = &state.phase else {
            panic!("expected the running phase after submitting");
        };
        match progress
            .outcome
            .as_ref()
            .expect("the run should have finished")
        {
            run_panel::RunOutcome::Finished { success, code } => {
                assert!(*success, "`echo` must succeed");
                assert_eq!(*code, Some(0));
            }
            run_panel::RunOutcome::StartFailed(e) => panic!("could not start `sh`: {e}"),
        }
        assert!(
            progress.log.iter().any(|line| line.contains("hello")),
            "the streamed output must contain the echoed text, got {:?}",
            progress.log
        );
    });
}

#[gpui::test]
fn run_command_that_fails_is_presented_as_a_completed_run_not_an_error(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    cx.simulate_keystrokes("cmd-e");
    cx.simulate_input("exit 3");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(2));

    view.read_with(cx, |app, _| {
        let state = app
            .run_command
            .as_ref()
            .expect("dialog should still be open, showing the running phase");
        let run_panel::RunPhase::Running(progress) = &state.phase else {
            panic!("expected the running phase after submitting");
        };
        match progress
            .outcome
            .as_ref()
            .expect("the run should have finished")
        {
            run_panel::RunOutcome::Finished { success, code } => {
                assert!(!success, "a non-zero exit is not a success");
                assert_eq!(
                    *code,
                    Some(3),
                    "the real exit code must be reported, not just pass/fail"
                );
            }
            run_panel::RunOutcome::StartFailed(e) => {
                panic!("a non-zero exit must never be presented as a start failure: {e}")
            }
        }
    });
}

#[gpui::test]
fn recent_command_survives_closing_the_run_dialog(cx: &mut TestAppContext) {
    let fx = Fixture::new();
    let repo = fx.open();
    let (view, cx) = open_app(cx, Some(repo));
    cx.run_until_parked();

    cx.simulate_keystrokes("cmd-e");
    cx.simulate_input("echo one");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(2));

    // Close the finished run — the suggestion list is read from
    // `WtmApp::recent_commands`, which outlives the dialog itself (session
    // state, not dialog state), so this must still show up after reopening.
    cx.simulate_keystrokes("escape");
    view.read_with(cx, |app, _| {
        assert!(app.run_command.is_none(), "escape closes the dialog");
    });

    let repo_path = fx.root().to_path_buf();
    view.read_with(cx, |app, _| {
        let recent = app
            .recent_commands
            .get(&repo_path)
            .expect("the repository must have a recent-commands entry after one run");
        assert_eq!(recent, &vec!["echo one".to_string()]);
    });
}

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **App: visual redesign.** The desktop app's colors, type, icons, and
  motion are rebuilt from a real design system instead of one-off
  values scattered across the codebase. Light mode is designed on its
  own terms rather than an inverted dark mode. The app now bundles its
  own typeface (Geist and Geist Mono) instead of relying on the
  platform default, so it looks the same on Linux as it does on macOS.
  The icon set grows from 15 to 43, and animations follow a
  deliberate, restrained catalog — a new **Reduce motion** setting
  turns them off and is remembered across restarts. **Caveat:**
  verified on macOS; the Linux rendering path (fonts especially) is
  covered by CI builds and tests but has not yet been run on a real
  Linux desktop by the maintainer.

### Added

- **App: dirty file counts.** Every worktree row now shows how many
  files are dirty (`3 dirty`), and the detail panel and Changes tab
  show the same count. `wtm list --json` gains a `dirty_count` field
  alongside the existing `dirty` boolean.
- **App: a scrollbar**, in every scrollable region — there wasn't one
  anywhere before.
- **App: keyboard navigation.** Tab moves focus around the app with a
  visible focus ring, and an open dialog traps Tab inside it instead
  of leaking focus to the app behind it.
- **App: the detail panel auto-collapses** on narrow windows to keep
  the worktree list usable, and restores itself once there's room
  again.
- **App: empty states offer their next action directly** (e.g. "Add
  Repository") instead of just describing what's missing.

### Fixed

- **App: selecting a worktree could silently select a different one.**
  Under Status or Recent sort, a background refresh that reordered the
  list kept the selection by row position rather than by worktree
  identity — the highlighted row could quietly become a different
  worktree, with a destructive action (Remove) one keystroke away. The
  selection is now looked up by worktree path after every reload.
- **App: scrolling inside a dialog, menu, or the command palette also
  scrolled the list behind it.** Overlays now block the scroll wheel
  from reaching whatever they're covering.
- **App: arrow-key selection didn't scroll the list to follow it**, so
  ↑/↓ could walk the highlight off either edge of the visible list.
- **App: the repository sidebar reordered itself when you selected a
  repo** (it sorted most-recently-opened first). It's now stable and
  alphabetical; the CLI still uses recency to pick a default repo.
- **App: toolbar controls (the filter field, the sort control) were
  unreachable at the smallest supported window size.**
- **App: the Changes tab's diff bodies hijacked vertical scrolling**
  and jittered sideways instead of scrolling the tab normally.
- **App: long paths, branch names, and commit subjects were clipped
  mid-word** instead of ellipsised, and could split an emoji or an
  accented character in half.
- Bumped `h2` to 0.4.16 for RUSTSEC-2026-0258 (transitive, through
  gpui's HTTP client; low-severity denial of service).

## [0.6.0] - 2026-08-16

### Added

- **App: Linux support.** `wtm-gui` now runs on Linux (X11 and Wayland
  sessions), not just macOS. Install it from a self-contained tarball
  (`WTM-linux-x86_64.tar.xz`/`WTM-linux-aarch64.tar.xz`, with a bundled
  `install.sh` and a `.desktop`/icon set for your app launcher) or a `.deb`
  (`WTM-linux-x86_64.deb`/`WTM-linux-aarch64.deb`) built from
  `crates/wtm-gui/Cargo.toml`'s new `[package.metadata.deb]` table. Desktop
  integrations are native rather than macOS shims: Reveal in Finder goes
  through the freedesktop `FileManager1` D-Bus interface (falling back to
  `xdg-open`), Open in Terminal honors `$WTM_TERMINAL` and then tries a list
  of common terminal emulators, and Copy Path uses whichever of
  `wl-copy`/`xclip`/`xsel` is installed. `wtm`'s CLI-side app launcher
  (`wtm`/`wtm app`) looks for the installed binary in the matching Linux
  locations. **Caveat:** this is verified in CI only — it builds, passes its
  test suite, and passes Clippy on `ubuntu-latest` and `ubuntu-24.04-arm` —
  it has not yet been run on a real Linux desktop by the maintainer.

## [0.5.0] - 2026-08-16

### Added

- **App: Fetch** (`⌘⇧F`, a toolbar button, and the empty-space context
  menu). Ahead/behind counts and prune's "upstream gone" detection are only
  ever as fresh as the last fetch — without this, the app could confidently
  show a worktree as "20 behind" long after that stopped being true, and
  wouldn't notice a deleted upstream branch until someone happened to fetch
  from a terminal. It runs `git fetch --prune` and reloads the list
  afterward, so those numbers actually change. It shells out to `git`
  rather than using `git2` directly, specifically so SSH agents, keychains,
  and `credential.helper` keep working — anything using authenticated
  transport, not just unauthenticated HTTPS. Needs network access and uses
  whatever git credentials are already configured for the repository.
- **App: worktree activity and sorting.** Each row now shows its
  last-commit age, and the list can be sorted by Name, Recent, or Status
  from a new control in the list toolbar. The main worktree stays pinned
  first in every mode. The chosen sort mode is session-only for now — it
  resets to Name on restart.
- **App: run a command in a worktree** (`⌘E`). A dialog streams the
  command's output live, remembers recently-run commands per repository as
  one-click suggestions, and shows a non-zero exit as a completed run
  rather than an error. Closing the dialog does not stop the command — it
  keeps running in the background, and the dialog's footer says so.
  Recent-command suggestions are session-only and reset on restart.
- **App: Open on Remote.** Turns a worktree's branch into its
  GitHub/GitLab/Bitbucket URL and opens it in the system browser. Disabled
  with a reason when the worktree is a detached HEAD or the repository has
  no resolvable remote.
- **App: command palette catch-up.** Fetch, Add Repository, the three
  detail-panel tabs (Details/Files/Changes), Run Command, and Open on
  Remote are now all reachable from `⌘K` — they had shortcuts before this
  but were undiscoverable without one.

## [0.4.0] - 2026-08-16

### Added

- **App: discoverable repository/worktree actions.** A `+` button on the
  sidebar's Repositories header opens a folder picker to add a repository
  (also bound to `⌘⇧O`), matched by an entry in the sidebar's empty state
  and in a new right-click menu on empty list space. The worktree list's
  toolbar gained New Worktree and Prune buttons, the latter showing a live
  count of prunable worktrees. Right-click context menus now cover the full
  action set on worktree rows, sidebar repositories, and empty list space,
  each item labeled with its keyboard shortcut.
- **App: mouse-driven multi-select.** A checkbox appears on each worktree
  row — on hover, or on every row once a multi-selection is active — plus
  an "N selected" bar with Remove Selected and Clear buttons. Shift-click
  and `⌘`-click still work exactly as before; the checkbox is another way
  in, not a replacement.
- **App: a base-ref picker for New Worktree.** The Base field is now a
  searchable picker listing local and remote-tracking refs, each tagged
  `current`/`default`/`worktree`/`remote` — `origin/main` and local `main`
  show up as separate, selectable entries instead of one deduplicated
  guess — while still accepting typed free text for a sha or a ref the
  picker doesn't list.
- **App: Files and Changes tabs on the detail panel**, beside Details
  (`⌘1`/`⌘2`/`⌘3`). Files is a lazily-expanding, gitignore-aware tree of the
  worktree's contents; Changes renders the worktree's uncommitted diff
  inline, with line-number gutters. The panel widens from 320px to 640px on
  these two tabs to give a diff room to be readable.
- `crates/wtm-gui`: a headless integration-test suite (`cargo test -p
  wtm-gui`), driving the real app through simulated keystrokes and clicks
  against a real temporary git repository to check flows like create,
  remove, prune, multi-select, and the command palette end to end.

### Fixed

- **App: Reveal in Finder always failed when the target didn't exist.**
  Both of `wtm`'s config files are optional and typically absent on a
  default install, and revealing a path that isn't there used to just
  fail outright. It now reveals the nearest existing ancestor directory
  instead.
- **App: the Settings sheet showed config paths as a bare "…".** A flex
  column was missing `flex_1`, so the path label had no claim on the row's
  width and collapsed to nothing next to its button. Paths now render
  home-relative, with a "Not created" note when the file doesn't exist yet.
- **App: a fast worktree create could leave the New Worktree dialog stuck
  showing progress.** A create with little or nothing to set up could
  finish before the dialog's progress screen noticed — the completion
  message could go unread — leaving the dialog open with nothing left to
  wait for. Both the create dialog's progress polling and the text field's
  cursor blink now use a timer the app reliably wakes up for.

## [0.3.0] - 2026-08-16

### Added

- **Desktop app** (`crates/wtm-gui`, ships as `WTM.app`): a native macOS app
  built with GPUI (the UI framework behind Zed). Multi-repo sidebar,
  worktree list with live status, create/remove/prune dialogs, a filesystem
  watcher that refreshes the list without a manual reload, a detail panel
  (upstream, path, HEAD, dirty files, recent commits), a ⌘K command palette
  with fuzzy search over worktrees and actions, type-to-filter,
  multi-select (shift/⌘-click) with bulk remove, right-click context menus,
  and a settings sheet (appearance, read-only effective repo config,
  generated keyboard-shortcut reference). Build it with
  `scripts/bundle-mac.sh`, or download `WTM-macOS.zip` from a release; it is
  ad-hoc signed, not notarized, so first launch needs right-click → Open.
  macOS only for now.
- **`wtm app`** (alias `gui`): open the desktop app explicitly, erroring
  (rather than falling back to the TUI) if it isn't installed.
- **Repository registry** (`~/.config/wtm/repos.json`, `src/registry.rs`):
  the list of repositories the desktop app's sidebar remembers, most
  recently opened first. A convenience cache only — worktrees are still
  always discovered from git's own registry, never from this file — so a
  corrupt or missing registry degrades to an empty sidebar rather than an
  error. Not read by the CLI.
- `setup::run_streaming`: a second entry point into post-create setup
  automation, alongside the existing `setup::run`, that reports each
  copy/command step (and captured command output) through a callback
  instead of inheriting stdout/stderr — for callers with no terminal to
  inherit into. The desktop app's create-worktree dialog streams its
  progress log through this.

### Changed

- **Bare `wtm` on a terminal now opens the desktop app** instead of the TUI,
  falling back to the TUI when the app isn't installed (or, with a warning,
  when it fails to launch). Piped/non-TTY invocations are unchanged: they
  still print help and exit `0`. `wtm tui` continues to launch the terminal
  UI directly regardless of what's installed.
- `commands::add::{CreateRequest, create}`,
  `commands::prune::{PruneCandidate, PruneReport, candidates,
  selection_candidates, execute}`, `commands::remove::{remove_worktree,
  is_dirty}`, and `commands::open::spawn_editor` are now `pub` (previously
  crate-private), so the desktop app can call the same cores the CLI and
  TUI already share instead of reimplementing them.
- `config::global_config_path` is now built on a new, also-`pub`
  `config::global_config_dir`, which the repository registry and the
  desktop app's own `gui.json` preferences file share with the CLI's
  `config.toml`.

## [0.2.2] - 2026-08-10

### Security

- Repository-shared `.worktree.toml` files can no longer supply executable
  editor or setup commands; those values are restricted to global or local
  configuration.
- Setup copy paths are contained within the repository and new worktree, and
  recursive copies reject symlinks instead of following them.
- The shell directory-change handoff only writes to wrapper-created temporary
  files without following symlinks.

### Fixed

- Explicit base refs now fail clearly when invalid, the main worktree is never
  labeled merged, and unavailable status scans are no longer shown as clean.
- TUI refresh generations prevent stale row/detail results, failures settle
  loading state, and prune discloses dirty worktrees with an explicit force
  toggle.
- Pruning continues independent candidates after recoverable failures and
  always attempts final registry cleanup.
- Quiet add/remove suppress Git progress and success text; editor launch
  validates the command; `add --cd` changes directory only after setup passes.
- Shell path handoff preserves non-UTF-8 and newline-containing Unix paths.

### Performance

- Release-mode list auditing now measures first load and an eleven-run warm
  median across 64 linked worktrees.

### Changed

- Updated Ratatui to 0.30 and aligned Crossterm 0.29, removing the vulnerable
  `lru` and unmaintained `paste` transitive dependencies. The MSRV is now 1.88.

## [0.2.1] - 2026-07-21

### Fixed

- **TUI: force-delete no longer silently quits.** Pressing `d` while the
  cursor was on the main worktree only showed a footer note and left the list
  in normal mode, so a `d f Enter` burst fell through to the `Enter` = "switch
  worktree and quit" binding — exiting the TUI (and, via the cd-on-exit
  wrapper, dropping you back at the shell) without removing anything. A
  rejected `d` now opens a dismissible notice modal that absorbs the follow-up
  keystrokes instead of letting them leak into normal mode.

### Changed

- Corrected the package `repository` metadata to the actual repository
  (`codenameakshay/wtm-manager`); the previous value produced broken install
  and download links in release artifacts.

## [0.2.0] - 2026-07-21

### Added

- **Full-screen TUI** (`wtm tui`, alias `wtm ui`; bare `wtm` also launches it
  on a TTY, printing help and exiting 0 in non-TTY contexts instead): left
  pane lists all worktrees with status badges (branch, short HEAD,
  ahead/behind, dirty, merged/gone/missing markers), right pane shows
  details (upstream, path, HEAD, dirty files, recent commits). Status loads
  in the background so launch is instant. Keybindings: `j`/`k`/`↓`/`↑` move
  selection, `g`/`G` jump to first/last, `Enter` switch (cd on exit), `n`
  new worktree, `d` remove (confirm; force if dirty), `Space` multi-select,
  `p` prune merged/gone/missing (or selection) with confirm, `o` open in
  editor, `x` run a command, `y` copy path, `/` fuzzy filter, `r` refresh
  status, `?` help overlay, `q`/`Esc` quit.
- **Bundled Agent Skill** at `skills/wtm/` (`SKILL.md`, `reference.md`,
  `scripts/install.sh`) teaching coding agents to install and drive `wtm`
  non-interactively.

### Changed

- **Shell wrapper**: replaced the old stdout-capture-based `wtm init`
  wrapper with a unified cd-on-exit mechanism built on a `$WTM_CD_FILE` temp
  file, so `cd`-on-switch works uniformly for plain commands and for the
  full-screen TUI (which owns the terminal and can't have its stdout
  captured). `wtm add --cd` and `wtm switch` now write the resolved path to
  `$WTM_CD_FILE` when the wrapper is active, instead of relying on stdout
  capture.

## [0.1.0] - 2026-07-21

Initial release.

### Added

- **Registry-based discovery**: `wtm` enumerates worktrees straight from git's
  own registry rather than a bespoke database, so it works with worktrees
  created anywhere — including via raw `git worktree add` — regardless of
  where they live on disk.
- **Commands**:
  - `add` (aliases: `new`, `create`) — create a worktree for a new or
    existing branch, with `--from`, `--path`, `--cd`, `--open`, and
    `--no-setup`.
  - `list` (alias: `ls`) — enumerate all worktrees with parallel status
    computation, with `--json` and `--no-status`/`--fast`.
  - `remove` (alias: `rm`) — remove a worktree safely, with `--force` and
    `--with-branch`.
  - `switch` (aliases: `cd`, `sw`) — change directory into a worktree via the
    shell wrapper.
  - `prune` (alias: `clean`) — clean up missing, merged, and gone-upstream
    worktrees, with `--merged`, `--gone`, `--dry-run`, `--force`.
  - `open` — launch an editor (or `--with` a custom command) in a worktree.
  - `path` — print a worktree's path (scripting-friendly, no interactive
    picker).
  - `init` — emit the shell integration function for `zsh`/`bash`.
  - `completions` — generate shell completion scripts.
  - `config path` / `config init` — inspect and scaffold layered configuration.
- **Performance-first `list`**: status (dirty/ahead/behind/upstream-gone/merged)
  is computed in parallel via `rayon`, one `git2::Repository` open per
  worktree; `--no-status`/`--fast` skips this entirely for near-instant
  enumeration. Target budget: `wtm list` under ~50ms for 10-20 worktrees on
  a warm cache.
- **Layered configuration**: built-in defaults, `~/.config/wtm/config.toml`,
  `<repo>/.worktree.toml`, `<repo>/.worktree.local.toml`, then CLI flags —
  each layer overriding the previous field-by-field.
- **Path templates** for new worktree placement (`{repo}`, `{branch}`,
  `{slug}`, `{home}`, `{repo_dir}` placeholders); templates affect only where
  *new* worktrees are created, never how existing ones are discovered.
- **Post-create automation**: copy or symlink files into new worktrees and
  run setup commands, configured per-repo.
- **Safety model**: `remove`/`prune` refuse to touch a dirty worktree without
  `--force`; branches are never deleted without explicit `--with-branch` (or
  during `prune --merged`/`--gone`); `protected_branches` are never removed
  or deleted.
- **Shell integration**: `wtm init zsh` / `wtm init bash` emit a shell
  function so `wtm switch`/`sw`/`cd` and `wtm add --cd` can change the
  calling shell's working directory, plus completions wiring.
- **Interactive picker**: `remove`, `switch`, and `open` fall back to a fuzzy
  `inquire`-based picker when no name is given and stdin/stderr are TTYs.
- CI performance gate (`tests/perf_gate.rs`) asserting the full
  list-with-status pipeline stays well under budget, plus `criterion`
  benchmarks (`benches/list.rs`).

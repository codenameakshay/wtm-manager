# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

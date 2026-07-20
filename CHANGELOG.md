# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

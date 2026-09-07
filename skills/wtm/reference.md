# wtm reference

Exhaustive command, configuration, and workflow reference for `wtm`. See
`SKILL.md` for the short version.

## Global flags

Apply to every subcommand:

| Flag | Description |
|---|---|
| `-C, --repo <PATH>` | Operate on the repository at `PATH` instead of the current directory. |
| `--color <auto\|always\|never>` | Control colored output. `auto` (default) colors when stdout is a TTY and `NO_COLOR` is unset. |
| `-v, --verbose` | Verbose output. |
| `-q, --quiet` | Quiet output (conflicts with `-v`). |

Exit status: `0` on success, non-zero on any error (bad args, git errors,
refused unsafe operations, etc.). Bare `wtm` with no subcommand: on a TTY,
opens the desktop app — falling back to the TUI only when the app isn't
installed — and in a non-TTY context (agent shells, pipes, CI) prints help
and exits `0` instead. Agents should never invoke bare `wtm`; always use an
explicit subcommand.

## Commands

### `wtm add [branch]` (aliases: `new`, `create`)

Create a worktree for `<branch>`, creating the branch from a base ref if it
doesn't exist yet (checked out as-is, erroring if already checked out
elsewhere, if it does exist). `--unique` and `--detach` make `branch`
optional.

| Flag | Description |
|---|---|
| `--from <base>` | Base ref for a new branch. Falls back to `default_base`, then `origin/HEAD` / `origin/main` / `origin/master`, then `HEAD`. |
| `--unique [stem]` | Create `stem/<8 hex>` (default stem: `wtm`) and retry on collision. |
| `--detach [name]` | Detached HEAD, no branch. `name` is only used in the path template. |
| `--path <path>` | Explicit destination path, overriding the path template. |
| `--cd` | cd into the new worktree after creation (requires the shell wrapper; writes to `$WTM_CD_FILE` when active). |
| `--open` | Open the new worktree in the configured editor after creation. |
| `--no-setup` | Skip `setup.commands`/`setup.copy` for this worktree. |
| `--json` | Print one object on stdout (`ok`, `action`, `name`, `branch`, `path`, `detached`). Silences git/setup chatter. |

Refuses if the destination path already exists. Setup failures are reported
but the worktree is kept. `--json` on failure still prints `error:` on
stderr (no JSON envelope).

Example `--json` object:

```json
{
  "ok": true,
  "action": "add",
  "name": "wtm/a1b2c3d4",
  "branch": "wtm/a1b2c3d4",
  "path": "/abs/path",
  "detached": false
}
```

### `wtm list` (alias: `ls`)

List every worktree in the repo's registry (main first, then linked ones),
including missing (moved/deleted) worktrees.

| Flag | Description |
|---|---|
| `--json` | Emit a JSON array instead of a table. |
| `--no-status`, `--fast` | Skip per-worktree status computation for near-instant enumeration. |

#### `--json` shape

An array of objects, one per worktree:

```json
{
  "name": "feature/login",
  "path": "/home/user/project-worktrees/feature-login",
  "branch": "feature/login",
  "head": "a1b2c3d",
  "is_main": false,
  "is_missing": false,
  "is_locked": false,
  "lock_reason": null,
  "head_time": 1757260800,
  "is_prunable": false,
  "status": {
    "dirty": false,
    "dirty_count": 0,
    "ahead": 2,
    "behind": 0,
    "upstream_gone": false,
    "merged": false
  }
}
```

- `branch` is `null` for a detached HEAD; `head` / `head_time` are `null`
  when the oid does not peel. `head_time` is unix seconds and is still
  present with `--fast`.
- `lock_reason` is `null` when unlocked, and a string (possibly empty) when
  `is_locked` is true.
- `status` is `null` entirely when `--no-status`/`--fast` was passed.
- `ahead`/`behind` are `null` when there is no upstream.
- `dirty_count` is `0` whenever `dirty` is `false`.

### `wtm remove <name>` (alias: `rm`)

Remove a worktree. `<name>` matches a registry entry name, a branch name, or
an unambiguous substring of the **display name** (the branch, or the
registry name when HEAD is detached); omit for an interactive picker (requires
stdin/stderr TTYs — never rely on this from an agent).

| Flag | Description |
|---|---|
| `--force` | Remove even with uncommitted changes. |
| `--with-branch` | Also delete the branch after removal (refused for protected branches). |
| `--json` | Print one object (`ok`, `action`, `name`, `path`, `branch_deleted`). |

Refuses to remove the main worktree or the worktree containing the current
directory. A worktree whose directory is already gone is safely dropped
from the registry.

### `wtm switch <name>` (aliases: `cd`, `sw`)

Resolve `<name>` (or show the picker) and hand its path to the shell
wrapper to `cd` into. Writes to `$WTM_CD_FILE` when the wrapper is active;
otherwise prints the path plus a hint to run `wtm init`. There is a hidden
`--print-path` flag used internally by the shell wrapper — don't rely on it
directly; use `wtm path <name>` instead.

### `wtm prune` (alias: `clean`)

Clean up worktrees that no longer need to exist: missing-directory or
git-prunable entries (always), plus opt-in merged/upstream-gone branches.

| Flag | Description |
|---|---|
| `--merged` | Include worktrees whose branch is merged into the resolved base. |
| `--gone` | Include worktrees whose upstream branch was deleted remotely. |
| `--detached` | Include linked worktrees whose HEAD is detached (no branch to delete). |
| `--dry-run` | Print the plan and exit without changing anything. |
| `--force` | Proceed even if a candidate worktree is dirty. |
| `--json` | Print one object (`ok`, `action`, `removed`, `skipped`, `failures`, `candidates`). Dry-run sets `removed` to 0. |

The main worktree and `protected_branches` are never candidates. Branches
for merged/gone candidates are deleted as part of pruning; missing-directory
entries only lose their registry entry. Always finishes with
`git worktree prune`.

### `wtm fetch`

Run `git fetch --prune` against the default remote (`origin` if configured,
otherwise the first remote name alphabetically) so ahead/behind counts and
`wtm prune --gone` see a current picture. Shells out to `git` so SSH
agents and `credential.helper` keep working.

| Flag | Description |
|---|---|
| `--remote <name>` | Fetch this remote instead of the default. |

### `wtm open [name]`

Open a worktree in the editor (resolves `<name>`, or shows the picker if
omitted).

| Flag | Description |
|---|---|
| `--with <cmd>` | Run `<cmd>` (via `sh -c`) in the worktree instead of launching an editor; waits for it and propagates its exit status. |

Editor resolution order: `config.editor` > `$VISUAL` > `$EDITOR`.

### `wtm path [name]`

Print a worktree's path and nothing else — no interactive picker, ever.
Safe to use from scripts and agents. Omit `name` to print the path of the
worktree containing the current directory.

### `wtm app` (alias: `gui`)

Open the desktop app explicitly, whatever the terminal context (not gated
on a TTY, unlike bare `wtm`). Errors with a non-zero exit and an actionable
message (naming `wtm tui` as the terminal alternative) if the app isn't
installed, rather than falling back automatically. **Never invoke this from
an agent** — it opens a GUI window.

### `wtm tui` (alias: `ui`)

Launch the full-screen interactive TUI directly. See [TUI](#tui) below.
Bare `wtm` no longer goes straight here: on a TTY it opens the desktop app
first, falling back to the TUI only when the app isn't installed. Never
invoke `wtm tui`, `wtm ui`, `wtm app`, `wtm gui`, or bare `wtm` in a
non-TTY context (agents, pipes, CI) — or from an agent at all — expecting
a UI.

### `wtm init <zsh|bash>`

Print the shell integration snippet: the `wtm` wrapper function plus
completion loading. See [Shell wrapper](#shell-wrapper).

### `wtm completions <zsh|bash>`

Print a shell completion script for the given shell.

### `wtm config path` / `wtm config init`

`wtm config path` prints the global config file path and, inside a repo,
the repo-level config paths, each annotated with whether it exists.

`wtm config init` scaffolds a fully commented `.worktree.toml` at the repo
root (errors if one already exists).

## Configuration

Layered, each layer overriding the previous **field-by-field** (list-valued
fields — `setup.commands`, `setup.copy`, `prune.protected_branches` — are
replaced wholesale by whichever layer last sets them, not merged):

1. Built-in defaults
2. `~/.config/wtm/config.toml` (global; honors `$WTM_CONFIG_DIR` and
   `$XDG_CONFIG_HOME/wtm`)
3. `<repo>/.worktree.toml` (checked in, shared with the team)
4. `<repo>/.worktree.local.toml` (git-ignored, machine-local)
5. Command-line flags

Keys (all optional):

```toml
# Destination path template for NEW worktrees created by `wtm add`. Only
# affects placement of new worktrees, never discovery of existing ones
# (which always comes from git's own registry).
#
# Placeholders: {repo} {branch} {slug} {home} {repo_dir}
path_template = "../{repo}-worktrees/{branch}"

# Base ref for "merged" detection (list/prune --merged) and the default
# base for `wtm add` when the branch doesn't exist yet. Unset tries
# origin/HEAD, origin/main, origin/master, then HEAD (no implicit fetch).
default_base = "origin/main"

# Editor for `wtm open`. Resolution at use time: config > $VISUAL > $EDITOR.
editor = "code"

[setup]
# Shell commands run via `sh -c`, in cwd = new worktree, right after
# creation. First failure stops the rest; the worktree is kept either way.
commands = ["npm install"]

[[setup.copy]]
path = ".env"
mode = "copy"      # "copy" | "symlink" (symlink targets the ABSOLUTE source path)

[[setup.copy]]
path = ".envrc"
mode = "symlink"

[prune]
# Never deleted, never selected as prune candidates.
protected_branches = ["main", "master", "develop"]
```

## TUI

Invoke directly with `wtm tui`/`wtm ui`. On a TTY, bare `wtm` opens the
desktop app first and only falls back to the TUI when the app isn't
installed — it no longer goes straight to the TUI. Status (dirty,
ahead/behind, merged, upstream-gone) loads in the background so launch is
instant — the list appears immediately and status badges fill in as they
resolve.

Layout: left pane lists all worktrees with status badges (branch, short
HEAD, ahead/behind, dirty, merged/gone/missing markers); right pane shows
details for the selected worktree (upstream, path, HEAD, dirty files,
recent commits).

Keybindings:

| Key | Action |
|---|---|
| `j`/`k` or `↓`/`↑` | Move selection |
| `g` / `G` | Jump to first / last |
| `Enter` | Switch to worktree (cd on exit, via shell wrapper) |
| `n` | New worktree (branch + base form; runs setup automation) |
| `d` | Remove selected worktree (confirm; force required if dirty) |
| `Space` | Toggle multi-select |
| `p` | Prune merged/gone/missing (or multi-selected rows) with confirm |
| `o` | Open in configured editor |
| `x` | Run a command in the worktree |
| `y` | Copy worktree path |
| `/` | Fuzzy filter |
| `r` | Refresh status |
| `?` | Help overlay |
| `q` / `Esc` | Quit |

## Shell wrapper

A `wtm` binary process can never change its parent shell's working
directory. `wtm init <zsh|bash>` prints a shell function (installed via
`eval "$(command wtm init zsh)"` / `... bash`) that:

1. Creates a temp file and exports its path as `$WTM_CD_FILE`.
2. Runs the real `wtm` binary with the original arguments.
3. If the binary wrote a path into that file, `cd`s into it.
4. Cleans up the temp file and preserves the binary's exit status.

`wtm switch`, `wtm add --cd`, and the TUI's `Enter` action all write to
`$WTM_CD_FILE` when it's set; without the wrapper installed, they just
print the target path (plus, for `switch`, a hint to run `wtm init`). This
replaced the old stdout-capture-based wrapper design — the new mechanism
works uniformly for plain commands and for the full-screen TUI, which owns
the terminal and can't have its stdout captured that way.

The function also wires up completions (loaded via `$fpath` for zsh,
`eval`'d directly for bash).

## Common workflows

**Spin up a throwaway worktree (agents):**
```sh
wtm add --unique --json
cd "$(wtm path wtm/<hex-from-json>)"
```

**Spin up a worktree for a branch:**
```sh
wtm add feature/login --json          # from default_base / origin / HEAD
wtm add --from origin/main hotfix/urgent --json
```

**Jump between worktrees (shell wrapper installed):**
```sh
wtm switch feature/login   # or: wtm cd feature/login / wtm sw feature/login
```

**From an agent (no shell wrapper in effect):**
```sh
cd "$(wtm path feature/login)"
```

**Clean up merged/gone/detached worktrees:**
```sh
wtm prune --merged --gone --detached --json --dry-run
wtm prune --merged --gone --detached --json
```

**Bring `.env` into every new worktree automatically:**
```toml
# .worktree.toml
[[setup.copy]]
path = ".env"
mode = "copy"
```
Then `wtm add <branch>` copies it in after creation (skip with `--no-setup`).

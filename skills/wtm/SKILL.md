---
name: wtm
description: Manages Git worktrees with the wtm CLI — installing wtm, creating/listing/switching/removing/pruning worktrees, running the wtm TUI, and setting up shell integration for cd-on-switch; fires on requests like "install wtm", "use worktrees", "create a worktree for branch X", "clean up merged worktrees", "switch worktree", "list my worktrees", or "set up wtm shell integration".
---

# wtm — fast Git worktree manager

`wtm` discovers worktrees straight from git's own registry (works no matter
where they live on disk), computes status in parallel, and exposes a small,
scriptable command set plus an optional full-screen TUI and a native desktop
app — neither of which an agent should ever launch (see the warning in
[Core usage for agents](#2-core-usage-for-agents) below).

## 1. Install

First check `git` is available (`git --version`); if not, tell the user to
install Git before continuing — wtm requires it.

Then check whether `wtm` is already installed: `wtm --version`. If present,
skip to shell integration below.

Otherwise pick the best available method, in order:

1. **cargo**, if a Rust toolchain is on `PATH` (`cargo --version`):
   ```sh
   cargo install --git https://github.com/codenameakshay/wtm-manager --locked
   ```
2. **Prebuilt binary** via the cargo-dist shell installer, otherwise:
   ```sh
   curl --proto '=https' --tlsv1.2 -LsSf https://github.com/codenameakshay/wtm-manager/releases/latest/download/wtm-installer.sh | sh
   ```

Homebrew support is planned but there is no published tap yet. The crate name
`wtm` on crates.io belongs to a different project, so always use the git URL
above when installing from source.

`skills/wtm/scripts/install.sh` automates the available installation paths
(detection, install, and shell integration) — prefer running it over doing
these steps by hand. After installing, verify with `wtm --version`.

### Shell integration

`wtm switch`/`wtm add --cd`/the TUI's Enter action can only change the
calling shell's directory through a small shell wrapper function — a plain
subprocess can never `cd` its parent shell. **With user confirmation**,
detect the user's shell (`$SHELL`), then idempotently append to the
matching rc file (grep first — never duplicate):

- zsh (`~/.zshrc`): `eval "$(command wtm init zsh)"`
- bash (`~/.bashrc`): `eval "$(command wtm init bash)"`

Tell the user exactly what line was added to which file, and that they need
to open a new shell (or `source` the rc file) for it to take effect.

## 2. Core usage for agents

| Task | Command |
|---|---|
| Create a worktree for a branch | `wtm add <branch>` (`--from <base>` to branch off something other than the default base) |
| List worktrees, machine-readable | `wtm list --json` |
| List worktrees, fast (no status) | `wtm list --no-status` |
| Get a worktree's path | `wtm path <name>` |
| Remove a worktree | `wtm remove <name> --force` |
| Clean up merged/gone worktrees | `wtm prune --merged --gone` (add `--dry-run` to preview) |
| Open a worktree in the editor | `wtm open <name>` |

**IMPORTANT for agents:**

- Always use non-interactive subcommands with explicit arguments and
  `--json`/`--no-status`/`--force` as needed — never rely on the
  interactive picker (it requires a TTY and will hang or fail otherwise).
- **Never invoke bare `wtm`, `wtm tui` (alias `wtm ui`), or `wtm app`
  (alias `wtm gui`) expecting a UI.** On a terminal, bare `wtm` now opens
  the native desktop app — falling back to the TUI only when the app isn't
  installed — which is even more disruptive to an automated caller than the
  TUI alone used to be. `wtm tui`/`wtm ui` force the terminal UI and
  `wtm app`/`wtm gui` force the desktop app; neither is safe to run
  automatically. In a non-TTY context (agent shells, pipes, CI), bare `wtm`
  still detects that and prints help with exit 0 instead of opening
  anything — but don't rely on that as a safety net; always use an explicit
  subcommand (`wtm list --json`, `wtm add`, `wtm path`, …) instead of any of
  these three.
- `wtm switch <name>` only changes the calling shell's directory when the
  user's shell wrapper (see above) is installed and active — which is never
  the case inside an agent's own subprocess. From an agent, resolve the
  path with `wtm path <name>` and `cd` yourself instead of running
  `wtm switch`.

## 3. Full reference

For the complete command/flag/alias list, JSON output shape, configuration
file format, the TUI and its keybindings, and common end-to-end workflows,
read `reference.md` in this skill's directory.

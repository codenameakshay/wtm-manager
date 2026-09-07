# Agents: use `wtm` for Git worktrees

Never run bare `wtm`, `wtm tui`, `wtm ui`, `wtm app`, or `wtm gui`. Those
open a UI. Always pass a subcommand.

Do not use `wtm switch`. Resolve a path and `cd` in this process:

```sh
cd "$(wtm path <name>)"
```

| Task | Command |
|---|---|
| List | `wtm list --json` (add `--fast` when status is not needed) |
| Create a throwaway tree | `wtm add --unique --json` (optional stem: `wtm add --unique agent --json`) |
| Create a named branch | `wtm add <branch> --json` |
| Detached leftover | `wtm add --detach --json` |
| Path | `wtm path <name>` |
| Remove | `wtm remove <name> --json` (`--force` if dirty) |
| Preview leftovers | `wtm prune --merged --gone --detached --json --dry-run` |
| Sweep leftovers | `wtm prune --merged --gone --detached --json` |
| Refresh remotes first | `wtm fetch` |

`--json` on add/remove/prune prints one object on stdout. Failures print
`error:` on stderr and exit 1; there is no JSON error envelope.

The canonical skill is `skills/wtm/` in this repo (JSON field names live in
`skills/wtm/reference.md`). Copy that folder; do not fork a second copy.

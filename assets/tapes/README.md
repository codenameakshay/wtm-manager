# Terminal recordings

The GIFs in the project README are produced with [VHS](https://github.com/charmbracelet/vhs)
from the `.tape` scripts in this directory. They all run against a throwaway
demo repository so the output is deterministic and shows a realistic spread of
worktree statuses (ahead, dirty, merged, upstream-gone, diverged).

## Prerequisites

- [`vhs`](https://github.com/charmbracelet/vhs) (`brew install vhs`) — pulls in
  `ttyd` and `ffmpeg`.
- `wtm` on your `PATH` (e.g. `cargo build --release` and add `target/release`
  to `PATH`).

## Recording

1. Build the demo fixture (creates `~/.wtm-demo`, uses only per-repo git
   config — your global `~/.gitconfig` is never touched):

   ```sh
   ./demo-setup.sh ~/.wtm-demo
   ```

2. Render a tape (rebuild the fixture first if a previous run mutated it):

   ```sh
   vhs list.tape      # -> ../list.gif
   vhs tui.tape       # -> ../tui.gif
   vhs add.tape       # -> ../add.gif
   vhs switch.tape    # -> ../switch.gif
   vhs prune.tape     # -> ../prune.gif
   ```

Each tape sets a clean `PS1`, `cd`s into `~/.wtm-demo/acme`, and `eval`s the
shell wrapper (`wtm init bash`) in a hidden preamble, then runs the visible
demo. Delete `~/.wtm-demo` when you're done.

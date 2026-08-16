# The wtm desktop app

A deeper guide to `WTM.app`, the native macOS app in `crates/wtm-gui`. See
the [README](../README.md#app) for the short version — installing it, the
keyboard shortcut table, and where its state lives. This document covers
what the README doesn't have room for: what each piece of the app actually
does and why it's built the way it is.

The app shares all of its git and worktree logic with the CLI and the
TUI — `commands::add::create`/`create_streaming`, `commands::prune::{candidates,
selection_candidates, execute}`, `commands::remove::remove_worktree`, and
`commands::open::spawn_editor` are the same functions every frontend calls.
The app never talks to git directly and never rewrites `.worktree.toml` or
`config.toml`; it only reads the same layered configuration the CLI does.

## Sidebar and repositories

The sidebar lists every repository you've opened in the app, most recently
opened first, backed by a registry at `~/.config/wtm/repos.json`
(`src/registry.rs`). This is a convenience cache, not a source of truth:
worktrees are always discovered fresh from git's own registry, the same way
the CLI does it. A missing, corrupt, or unreadable registry file just means
an empty sidebar, never a startup failure.

A repository that no longer exists on disk (an unmounted drive, a deleted
directory) stays in the list, shown greyed out, rather than disappearing —
losing your list because a volume happened to be unplugged would be worse
than a stale entry. Right-click a sidebar entry for **Open**, **Reveal in
Finder**, **Copy Path**, or **Remove from Sidebar** — the last one only
forgets the registry entry; it never touches anything on disk.

**Adding a repository** — the `+` button next to the "Repositories" header,
`⌘⇧O`, or (when the sidebar is empty) the "Add Repository…" row in its own
empty state — opens a native folder picker. Choosing a directory resolves it
to a git repository, adds it to the registry, and selects it, the same
`activate_repo` path a sidebar click already takes; choosing something that
isn't a git repository reports that in the footer instead of adding a
useless entry. All three affordances funnel through the same handler, so
there's no difference in behavior between them, only in how you reach it.

Opening the app from the Dock or Spotlight (no repository argument) falls
back, in order: the current working directory's repository (rarely
meaningful for a Dock launch), then the last repository that was open,
recorded in `gui.json`.

## Worktree list, status, and live refresh

The list loads in two passes, the same as the TUI: an immediate listing
without status so the window isn't blank, then a second pass with
dirty/ahead/behind/merged status computed in the background. A filesystem
watcher (`src/watcher.rs`) then keeps it current without a manual ⌘R: it
watches the repository's `.git` directory recursively (branch switches,
commits, worktree add/remove all touch something under there) plus each
worktree's own root directory non-recursively (so a worktree relocation or
its `.git` file/directory changing is caught, without recursing into
`node_modules` or build output and firing on every file a build touches).
Object-database writes, the reflog, and `*.lock` files are filtered out as
noise — they fire on nearly every git operation without changing anything
the app shows. A burst of filesystem events (a `git commit`, a `worktree
add`) is debounced into a single refresh.

Watching can fail — a platform watch-descriptor limit, a permissions error —
and that's never surfaced as an error message: it just means live refresh is
unavailable for that repository and ⌘R keeps working normally.

The list toolbar carries labeled New Worktree and Prune… buttons above the
rows — the same actions ⌘N/⌘⇧P and the command palette already reach, made
into a visible door instead of something only a shortcut table names. The
Prune… button's label includes a live count of prunable worktrees (using
the same baseline candidate rules — nothing merged/gone-specific until you
open the dialog and turn those toggles on), so there's a reason to click it
beyond curiosity.

## Create, remove, and prune

**Create** (⌘N) is a two-phase dialog: fill in a branch name (with a
filtered picker of existing branches below it, showing which are already
checked out elsewhere or have a gone upstream) and an optional base ref,
then submit. The Base field doubles as a searchable ref picker: typing (or
just focusing the field) shows local branches, remote-tracking branches, and
two synthetic entries — `current` (whatever the worktree you were looking at
has checked out) and `default` (the repo's configured `default_base`, or
`HEAD`) — each labeled with what it is. `origin/main` and local `main` are
kept as separate, selectable entries rather than deduplicated into one
guess, since branching from one or the other is a real, different choice.
The field still accepts typed free text for a sha or a ref the picker
doesn't list; Escape closes the picker without closing the dialog underneath
it, and Enter picks whatever's highlighted or, if nothing matches, submits
exactly what you typed.

Once submitted there's no going back to the form — the dialog
switches to a streaming progress log of the same setup automation
(`setup.commands`/`setup.copy`) the CLI runs, reported line-by-line as it
happens via `setup::run_streaming` (a second entry point into setup
alongside the CLI/TUI's `setup::run`, which inherits stdio instead of
capturing it — the app has no stdio to inherit into). A run-setup toggle is
disabled and explained, not hidden, when the repository has no setup
commands or copy entries configured. The worktree is created and kept even
if a setup command fails; the log says which one and why.

**Remove** (⌘⌫ or Delete) confirms before doing anything, refuses on the
main worktree unconditionally, and requires the Force toggle before
touching a dirty worktree. A "delete branch too" toggle is disabled with a
reason when the branch is in `prune.protected_branches`.

**Prune** (⌘⇧P) mirrors `wtm prune --merged --gone`: Merged and Gone toggles
recompute the candidate list live (the same selection logic as the CLI, so
the main worktree and protected branches are never candidates), a Force
toggle covers dirty candidates, and the confirm button reports counts
removed/skipped/failed honestly rather than claiming a uniform success.

Selecting more than one row (see Multi-select below) and pressing ⌘⌫ opens a
bulk-remove confirmation instead of the single-target dialog, built from the
same safety-filtered candidate list the Prune dialog uses.

## Detail panel

Toggled with ⌘I: three tabs — Details, Files, Changes (⌘1/⌘2/⌘3) — for the
selected worktree. The panel is 320px wide on Details and widens to 640px
on Files/Changes, since a diff in a 320px column isn't one anyone can read.

**Details** shows upstream, path, HEAD, dirty files, and recent commits —
the same facts the TUI's right-hand pane shows. Loaded in the background per
selection and discarded if the selection moves on before it arrives, so a
fast series of arrow-key presses never paints a stale worktree's details a
moment late.

**Files** is a lazily-expanding tree of the worktree's working directory:
one directory level loads per click, gitignore-aware, so an unexpanded
`node_modules` costs exactly one row instead of a walk of its contents.
Clicking a file shows its diff (if any) in a column beside the tree.

**Changes** renders every uncommitted file's diff for the worktree in one
scrolling column, inline, with line-number gutters — the same unified-diff
shape `git diff` output uses, drawn directly instead of shelling out to a
pager. A binary file, or a diff past the data layer's 2000-line-per-file
cap, says so explicitly rather than rendering empty or silently truncated.
Both tabs load through the same generation-counter discipline the Details
tab already used, so a slow listing for a worktree you've since navigated
away from can't overwrite what's currently on screen.

## Command palette

⌘K opens a fuzzy-search overlay over both the open repository's worktrees
and the app's own actions (New Worktree, Remove Worktree, Prune, Reload,
Open in Editor/Terminal, Reveal in Finder, Copy Path, Toggle Sidebar/Detail
Panel, Settings). The scorer favors matches at word boundaries — the start
of the string, or right after `/`, `-`, `_`, `.`, a space, or a
lowercase-to-uppercase transition — so a query like `mwg` lands on the
initials of `migrate`/`wtm`/`gpui` in a branch like
`migrate-wtm-to-gpui-app` rather than on some earlier, less meaningful
triple of letters. Plain Enter (or a plain click) selects a worktree result
and closes the palette; ⌘+Enter (or a ⌘-click) additionally opens it in your
editor. Command results ignore that modifier — there's no "jump vs. open"
distinction for running a command.

## Filtering and multi-select

⌘F focuses a type-to-filter field above the list; it narrows which rows are
visible without re-ordering them (the list's "main first" order is part of
what makes it scannable). Arrow keys move through whatever's currently
visible.

Shift-click selects every visible row between the anchor and the clicked
row; ⌘-click toggles one row in or out of the selection and moves the anchor
to it, the same "last-touched row is the anchor" convention Finder uses. A
small checkbox at the left edge of each row is the mouse-only equivalent:
hidden until you hover the row, except once a real multi-selection (two or
more rows) exists, when every row's checkbox stays visible so the whole
selection reads at a glance. Once that happens, an "N selected" bar appears
between the toolbar and the list with Remove Selected and Clear buttons —
the discoverable surface for the same bulk-remove path a multi-row ⌘⌫
already reaches.

When no dialog, overlay, or menu is open, Escape's last resort — instead of
doing nothing — is to collapse a multi-selection back to its anchor row;
that's the one thing Escape falls through to, since it's non-destructive,
consistent with Escape never falling through to anything that isn't.

## Context menus

Every right-click menu shows each item's keyboard shortcut alongside its
label — that's how a shortcut gets learned rather than looked up in this
document.

Right-clicking a worktree row selects it (unless a multi-selection is
already active, in which case the row is only described, not folded into
it) and opens Open in Editor (⏎), Open in Terminal (⌘⇧T), Reveal in Finder
(⌘⇧R), Copy Path (⌘C), a selection toggle labeled Select/Add to
Selection/Remove from Selection depending on the row's current state, and
Remove… (⌘⌫) — disabled, with "main worktree" in its shortcut slot, on the
main worktree.

Right-clicking the list's own empty space (not a row) opens New Worktree
(⌘N), Prune… (⌘⇧P), and Reload (⌘R) — shown but disabled when no repository
is open, rather than hidden, so an empty window's right-click never looks
broken — plus Add Repository… (⌘⇧O), which works either way.

Right-clicking a sidebar repository opens it and offers Open, Reveal in
Finder, Copy Path, and Remove from Sidebar — the last one only forgets the
registry entry, the same guarantee as the sidebar's own row menu above.

## Settings

⌘, opens a settings sheet with four sections:

- **Appearance** — System, Light, or Dark, persisted and applied
  immediately. Forcing Light or Dark survives a live OS appearance change;
  System keeps following it.
- **Terminal app** — read-only, showing whatever `$WTM_TERMINAL` currently
  resolves to (`Terminal` if unset). There's no in-app field for this
  because nothing downstream of one would currently read it; changing which
  terminal `⌘⇧T`/"Open in Terminal" uses means setting the environment
  variable. Also macOS-only for now, same as the app itself.
- **Effective repository configuration** — a read-only view of `wtm`'s own
  layered TOML config as it applies to the open repository (path template,
  default base, editor, protected branches, setup commands/copy entries).
  Both the global config and the repo config are optional and typically
  don't exist on a fresh install, so each is shown home-relative with a
  "Not created" note when its file is absent, next to a **Reveal** button —
  labeled "Reveal Folder" instead of "Reveal" when the file itself isn't
  there, since Reveal in Finder's own missing-path fallback would land on
  the containing folder, not a file that doesn't exist. This is
  deliberately not editable here: it's the CLI's config, potentially
  checked into the repository, and the app must never rewrite it.
- **Keyboard Shortcuts** — generated from the same table `main.rs` uses to
  register the real key bindings (`cx.bind_keys`), so this list is
  structurally unable to drift from what's actually bound. See the
  [README](../README.md#app) for that table.

## Where its state lives

Two files, next to the CLI's own `~/.config/wtm/config.toml` (same
`$WTM_CONFIG_DIR`/`$XDG_CONFIG_HOME` overrides):

- `~/.config/wtm/repos.json` — the sidebar registry (`src/registry.rs`):
  each entry's path, display name, and last-opened timestamp.
- `~/.config/wtm/gui.json` — GUI-local preferences (`src/prefs.rs`):
  appearance, sidebar/detail-panel visibility, window frame, and last-opened
  repository path.

Both use the same persistence pattern: an atomic write (temp file, then
rename) and a schema version, so a crash mid-write can't truncate the file
and a file from a newer build of the app is ignored wholesale rather than
partially trusted. Neither file is read by the CLI — they exist purely for
the app's own sidebar and window state.

## Testing

`cargo test -p wtm-gui` runs a headless integration suite alongside the
crate's unit tests, driving the real app through gpui's `TestAppContext`
(simulated keystrokes and clicks against a real temporary git repository)
rather than launching it by hand or relying on screen capture, neither of
which has been reliable in this environment. It asserts against git's own
state, not just the absence of a panic: creating a worktree checks it shows
up in `git worktree list`, removing one checks it leaves disk, removing the
main worktree checks it's refused. Coverage includes startup, create
(including the base-ref picker), remove, prune, multi-select (both
shift/⌘-click and the bulk-remove path), filtering, the command palette, the
detail panel's Files/Changes tabs, adding a repository, and Escape's
layered close/collapse behavior.

Two things are deliberately not covered, and the suite says so rather than
faking it:

- **The filesystem watcher.** It's driven by real, non-deterministic OS
  filesystem events, which the headless dispatcher can't make reproducible —
  and starting a real one inside a test would block the dispatcher's single
  cooperative thread forever, since its consumer blocks on a channel `recv()`
  that's only ever safe on the real app's dedicated background thread. Tests
  disable the watcher and instead trigger the same `reload` it would
  eventually cause directly (⌘R, or a create/remove/prune completing).
- **The native folder picker** behind "Add Repository". gpui 0.2.2's
  `TestPlatform` has no way to simulate the open/choose-existing dialog
  `cx.prompt_for_paths` calls — it's `unimplemented!()` and panics if
  invoked. The resolve-and-activate logic that runs once a path comes back
  is still tested directly; only the platform picker call itself is
  untested.

## Platform support

macOS only. GPUI itself supports Linux and Windows, but this app has not
been built or tested on either — the CI gate for `crates/wtm-gui`
(`.github/workflows/ci.yml`) only runs on `macos-latest`, and
`scripts/bundle-mac.sh` refuses to run anywhere else.

The release build (`WTM-macOS.zip`, attached to GitHub Releases by
`.github/workflows/release-gui.yml`) and a bundle you build yourself with
`scripts/bundle-mac.sh` are both ad-hoc signed by default, not notarized —
see the [README](../README.md#app) for what that means for Gatekeeper on
first launch.

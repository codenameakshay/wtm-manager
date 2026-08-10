# Release Hardening Design

## Objective

Make the next `wtm` release safe to run in unfamiliar repositories, accurate
under concurrent TUI refreshes, predictable in scripts and shell wrappers,
accessible on the project website, and measurably fast with more than fifty
registered worktrees.

The accepted scope is the complete 2026-08-10 release audit. Compatibility
with unsafe or misleading behavior is intentionally not preserved.

## Security and configuration boundary

The version-controlled `.worktree.toml` remains useful for declarative shared
settings, but it may not contain executable `editor` or `setup.commands`
values. Executable values are accepted only from the user's global config or
`.worktree.local.toml`. Loading a shared config that contains them fails with
an actionable error instead of silently ignoring them.

Every `setup.copy.path` must be a non-empty relative path with no parent,
root, or platform-prefix component. Source resolution must remain inside the
main worktree, destination traversal must remain inside the new worktree, and
recursive copy rejects symlinks rather than following them. Unix destination
traversal and creation are descriptor-relative (`openat`/`mkdirat`) with
no-follow semantics on every component. Explicit
`mode = "symlink"` may link to a regular in-repository source, but cannot use a
source that resolves outside the main worktree.

## Git status and destructive operations

An explicitly configured base is strict. If it cannot resolve, listing,
creation, TUI status loading, and pruning return a clear error; they never
silently substitute another ref. With no configured base, `HEAD` remains the
documented default.

The main worktree is never labeled merged. A status scan failure produces an
unavailable status rather than a false clean result. Prune preflights all
candidates, reports per-item failures, completes safe independent work, and
always attempts the final registry prune. Gone-upstream cleanup keeps branches
unless branch deletion is explicitly requested by the existing prune mode and
the plan is visible to the user.

## CLI, shell, and editor behavior

`--quiet` suppresses wtm success text and Git progress. `add --cd` records a
directory change only after setup succeeds. Picker cancellation is a normal,
quiet exit. Empty config-directory environment variables fall through to the
normal config lookup. Editor opening performs a command preflight and reports
failure before printing success.

The shell handoff remains a private wrapper protocol, but paths are transported
without lossy display conversion. Unix tests cover non-UTF-8 paths; the wrapper
only changes directory after a successful command.

## TUI concurrency and interaction

Row and detail loads carry request generations. The state machine accepts only
the latest generation, preventing old threads from repainting stale data.
Success and failure both settle the loading indicator. Refreshes retain current
rows until the new full-status result arrives, so a fast no-status response
cannot overwrite a complete result.

Prune is unavailable until status data is ready. Dirty candidates are shown in
the confirmation dialog, and force is an explicit toggle passed to the shared
prune executor. The effect queue uses `VecDeque` and repeated refresh requests
are superseded rather than accumulated.

## Performance and release gates

The release-mode fixture contains 64 linked worktrees. It records the first
load and an eleven-run warm median, with explicit budgets suitable for noisy
CI. Criterion uses the same scale. CI runs formatting, clippy, tests,
documentation, RustSec auditing, an explicit Rust 1.88 check, and release
checks with `--locked`.

The package version, lockfile, changelog, README, website, and release claims
must agree before the release gate passes.

## Website and documentation

Documentation explains the new shared-config trust boundary, strict base
resolution, quiet behavior, prune behavior, and 64-worktree performance gate.
The website uses accessible contrast for small text, visible focus indicators,
semantic selected state for demo controls, announced dynamic demo/release
content, reduced-motion behavior, and a valid Pages environment URL.

## Verification

Every behavior change is protected by a regression test that is observed
failing before implementation. Completion requires the full test matrix,
clippy, formatting, rustdoc with warnings denied, shellcheck, RustSec audit,
the 64-worktree release gate, website static checks, `cargo check --locked`,
and a clean diff check.

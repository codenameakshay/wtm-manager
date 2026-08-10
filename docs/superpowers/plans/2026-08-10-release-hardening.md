# Release Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve the full release audit, update the website and documentation, and enforce a representative 64-worktree performance gate.

**Architecture:** Preserve the existing module boundaries while making unsafe configuration invalid, destructive decisions strict, and asynchronous TUI messages generation-aware. Shared command cores remain the only mutation path; the CLI, TUI, documentation site, and release automation consume the corrected behavior.

**Tech Stack:** Rust 2021, git2, clap, ratatui/crossterm, shell integration, static HTML/CSS/JavaScript, GitHub Actions, cargo-dist.

---

### Task 1: Lock release metadata and the 64-worktree performance contract

**Files:**
- Modify: `Cargo.lock`
- Modify: `CHANGELOG.md`
- Modify: `tests/perf_gate.rs`
- Modify: `benches/list.rs`
- Modify: `.github/workflows/ci.yml`

- [x] Write a failing release check demonstrating `cargo check --locked` rejects the current package/lock mismatch.
- [x] Change the performance fixture constant from 15 to 64, record first-load and warm-median measurements, and set explicit CI-tolerant budgets.
- [x] Run `cargo test --release --test perf_gate -- --ignored --nocapture` and record the 64-worktree timings.
- [x] Update the lockfile and changelog, then run `cargo check --locked` and require exit 0.
- [x] Add locked CI commands, rustdoc warnings-as-errors, RustSec auditing, and an exact Rust 1.88 MSRV job without weakening existing checks.

### Task 2: Enforce the configuration trust boundary

**Files:**
- Modify: `src/config.rs`
- Modify: `tests/cli_config_shell.rs`

- [x] Add regression tests where `.worktree.toml` contains executable `setup.commands` or `editor` values and assert configuration fails with instructions to move them to `.worktree.local.toml`.
- [x] Run the focused tests and confirm they fail because shared executable configuration was previously accepted.
- [x] Validate the shared layer before merge while continuing to accept executable values from global and local configuration.
- [x] Add and fail a regression test for `WTM_CONFIG_DIR=""` falling through to XDG/home resolution, then implement the empty-value check.
- [x] Run config unit and CLI integration tests to green.

### Task 3: Contain setup copy and symlink behavior

**Files:**
- Modify: `src/setup.rs`
- Modify: `src/error.rs` if a dedicated error improves the message

- [x] Add failing tests for absolute paths, `..`, empty paths, a source that resolves outside the main root, a destination-parent symlink, a recursive child symlink, and a symlink cycle.
- [x] Verify each test fails for the unsafe behavior rather than fixture setup.
- [x] Add one relative-path validator and root-containment helpers; reject symlinks during recursive copy and unsafe destination parents.
- [x] Bind Unix destination creation to no-follow directory descriptors so destination-parent swaps cannot escape the new worktree.
- [x] Preserve regular file/directory copy permissions and explicit symlink mode for contained regular sources.
- [x] Run setup tests and the setup CLI integration tests to green.

### Task 4: Make base and status computation strict and accurate

**Files:**
- Modify: `src/worktree.rs`
- Modify: `src/commands/add.rs`
- Modify: `src/output.rs`
- Modify: `tests/cli_json.rs`
- Modify: `tests/cli_remove_prune.rs`

- [x] Add failing tests showing an unresolved explicit base errors, local `main` tracking `origin/main` is not merged, and an unavailable dirty scan is not rendered as clean.
- [x] Change explicit base resolution to return an error and retain `HEAD` only for the no-base case.
- [x] Pass main-worktree identity into status computation and force its merged flag false.
- [x] Represent uncomputable status as unavailable in table/JSON behavior.
- [x] Run worktree, JSON, lifecycle, and prune tests to green.

### Task 5: Make prune execution complete and reportable

**Files:**
- Modify: `src/commands/prune.rs`
- Modify: `src/tui/app.rs`
- Modify: `src/tui/view.rs`
- Modify: `src/tui/mod.rs`
- Modify: `tests/cli_remove_prune.rs`

- [x] Add failing tests for a failure after one successful candidate, final registry-prune execution, dirty-candidate disclosure, live dirty-state rechecking, and TUI force propagation.
- [x] Introduce per-candidate outcomes in `PruneReport`; preflight protected/main/cwd constraints before mutation.
- [x] Recheck live dirty state immediately before removal and fail closed when status is unavailable.
- [x] Continue independent candidates after recoverable failures and always attempt `git worktree prune`, returning an aggregate error only after the report is complete.
- [x] Add a force toggle and unsafe-status warning to the TUI confirmation modal.
- [x] Run prune CLI and TUI state/view tests to green.

### Task 6: Remove TUI refresh races and loading glitches

**Files:**
- Modify: `src/tui/app.rs`
- Modify: `src/tui/mod.rs`

- [x] Add failing state-machine tests where an old row result and old detail result arrive after a newer generation.
- [x] Add a failing test showing a current-generation row failure clears `status_loading` while preserving existing rows.
- [x] Carry generations through row/detail effects and messages and ignore superseded results.
- [x] Stop scheduling paired fast/full refreshes after mutations; retain current rows while one full refresh runs.
- [x] Replace the effect vector front-removal path with `VecDeque`.
- [x] Prevent prune candidate selection while status is loading and run all TUI tests to green.

### Task 7: Correct CLI, editor, shell, and path behavior

**Files:**
- Modify: `src/gitcmd.rs`
- Modify: `src/commands/add.rs`
- Modify: `src/commands/remove.rs`
- Modify: `src/commands/open.rs`
- Modify: `src/commands/mod.rs`
- Modify: `src/main.rs`
- Modify: `src/cdfile.rs`
- Modify: `src/commands/init.rs`
- Modify: `src/commands/path.rs`
- Modify: `tests/cli_lifecycle.rs`
- Modify: `tests/cli_config_shell.rs`

- [x] Add failing integration tests for silent `-q add/remove`, setup failure with `--cd`, invalid editor startup, and quiet picker cancellation behavior at the error boundary.
- [x] Add Unix regression tests for non-UTF-8 cd-file paths and a wrapper status guard.
- [x] Make Git add output capturable in quiet mode, gate success messages, move cd handoff after setup, and preflight plain or quoted editor executables without rejecting legitimate shell forms.
- [x] Add a dedicated cancellation result handled as a successful no-op by the binary.
- [x] Make the cd handoff byte-safe on Unix and fast-path `wtm path` from the discovered current worktree.
- [x] Run all CLI and shell integration tests to green.

### Task 8: Resolve dependency and documentation quality warnings

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: public module documentation under `src/commands/` and `src/tui/mod.rs`

- [x] Determine the smallest maintained ratatui-compatible dependency update that removes the RustSec `lru` unsound warning and unmaintained `paste` warning.
- [x] Apply the update without adding an avoidable direct dependency or compatibility layer.
- [x] Run `cargo audit`, dependency-tree checks, and the full Rust suite.
- [x] Correct private intra-doc links and run `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features` to green.

### Task 9: Update the website and user documentation

**Files:**
- Modify: `README.md`
- Modify: `DESIGN.md`
- Modify: `docs/index.html`
- Modify: `docs/styles.css`
- Modify: `docs/script.js`
- Modify: `.github/workflows/pages.yml`

- [x] Update configuration examples so shared files are declarative and executable commands live in `.worktree.local.toml` or global config.
- [x] Document strict base errors, corrected quiet/cd behavior, TUI refresh/prune behavior, and the measured 64-worktree gate.
- [x] Raise small-text contrast to at least 4.5:1, add visible `:focus-visible` indicators, and expose demo selection through native buttons plus `aria-pressed` and an announced status region.
- [x] Correct the Pages environment URL expression and keep reduced-motion behavior intact.
- [x] Serve the site locally; verify keyboard traversal, selected-state updates, release fallback behavior, 320px reflow, and static asset requests.

### Task 10: Integrated release verification

**Files:**
- Review all modified files

- [x] Run `cargo fmt --all -- --check`.
- [x] Run `cargo clippy --all-targets --all-features --locked -- -D warnings`.
- [x] Run `cargo test --all-targets --all-features --locked`.
- [x] Run `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --locked`.
- [x] Run `cargo audit --file Cargo.lock` and review every warning.
- [x] Run `cargo test --release --test perf_gate --locked -- --ignored --nocapture` and record first-load plus median timings for 64 worktrees.
- [x] Run `shellcheck skills/wtm/scripts/*.sh`, generated Bash shell integration, JavaScript syntax validation, `git diff --check`, and review `git status --short`.
- [x] Re-read the design and audit checklist, inspect the final diff, and resolve every remaining discrepancy before completion.

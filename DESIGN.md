# wtm — internal design contract

This file is the authoritative interface contract between modules while the
project is being built. Public interfaces and behavioral invariants documented
here must stay aligned with the implementation; update this contract whenever
they change.

Conventions:
- Library code returns `crate::error::Result<T>` (`crate::Error`), never `anyhow`.
- `anyhow` is used only in `src/main.rs`.
- No panics on user error. `unwrap()` only where infallibility is locally provable.
- All reads via `git2`; process spawn of `git` ONLY in `src/gitcmd.rs`.
- Edition 2021, MSRV 1.88, `cargo fmt` default style, `clippy -D warnings`,
  rustdoc warnings, the performance gate, and dependency audit must pass.

## src/repo.rs — repository discovery

```rust
use std::path::{Path, PathBuf};
use crate::error::Result;

/// Resolved repository context. Holds paths only (git2::Repository is not
/// Sync; callers open repositories on demand, per thread).
#[derive(Debug, Clone)]
pub struct RepoContext {
    /// Absolute path to the MAIN working tree root (not a linked worktree).
    pub main_root: PathBuf,
    /// Absolute path to the main repository's .git directory (common dir).
    pub git_dir: PathBuf,
    /// Directory name of the main working tree (used as {repo} in templates).
    pub repo_name: String,
}

impl RepoContext {
    /// Open a git2 Repository for the main working tree.
    pub fn open_main(&self) -> Result<git2::Repository>;
}

/// Discover the repository from `start` (or the current directory), resolving
/// to the MAIN working tree even when invoked from inside a linked worktree or
/// any subdirectory. Errors: RepoNotFound, BareRepo.
pub fn discover(start: Option<&Path>) -> Result<RepoContext>;
```

Implementation notes: `git2::Repository::discover`; if `repo.is_worktree()`,
the main `.git` dir is `repo.commondir()` and `main_root` is its parent.
Otherwise `main_root = repo.workdir()` (error `BareRepo` if `None`).
Canonicalize paths (macOS `/private/tmp` vs `/tmp` must compare equal).

## src/worktree.rs — registry enumeration + status (perf-critical)

```rust
use crate::error::Result;
use crate::model::{WorktreeInfo, WorktreeStatus};
use crate::repo::RepoContext;

/// Options for listing.
#[derive(Debug, Clone, Default)]
pub struct ListOptions {
    /// Compute per-worktree status (dirty/ahead/behind/gone/merged) in
    /// parallel with rayon. When false, `WorktreeInfo.status` is None.
    pub with_status: bool,
    /// Base ref for merged detection, e.g. "origin/main" (config
    /// default_base). Resolved and peeled to a commit in the main repo. None
    /// uses the main worktree HEAD; an invalid explicit value is an error.
    pub base: Option<String>,
}

/// Enumerate ALL worktrees from git's own registry: the main worktree first,
/// then every linked worktree (wherever it lives on disk), including entries
/// created by raw `git worktree add`. Registry entries whose directory has
/// been moved/deleted are returned with `is_missing = true` (never an error).
/// Status computation uses rayon par_iter, opening a git2 Repository per
/// worktree; missing worktrees get `status: None`.
pub fn list(ctx: &RepoContext, opts: &ListOptions) -> Result<Vec<WorktreeInfo>>;

/// Resolve `<name>` to a worktree: exact match on registry name, then branch
/// name, then unique substring of branch/name (error WorktreeNotFound
/// otherwise; if substring matching is ambiguous, also WorktreeNotFound with
/// the candidates listed in the message). Never computes status.
pub fn find(ctx: &RepoContext, name: &str) -> Result<WorktreeInfo>;

/// Worktree containing `path` (used to detect "you are removing the worktree
/// you are standing in"). None if path is in no known worktree.
pub fn containing(ctx: &RepoContext, path: &std::path::Path) -> Result<Option<WorktreeInfo>>;
```

Status semantics (document + unit-test these in-module):
- dirty: `repo.statuses` with `include_untracked(true)`, ignored excluded,
  `exclude_submodules(true)` → any entry ⇒ dirty.
- ahead/behind: local branch upstream via `branch.upstream()`; when present,
  `repo.graph_ahead_behind`. No upstream ⇒ both None.
- upstream_gone: branch HAS upstream config (`branch.<name>.merge` set, check
  via config or `upstream()` returning NotFound while config exists) but the
  remote-tracking ref is gone ⇒ true.
- merged: branch tip is ancestor of (or equal to) resolved base tip, computed
  in the MAIN repo via `graph_descendant_of(base, tip)` or `merge_base == tip`.
  The main worktree and the base's own linked worktree are NOT flagged merged.
- A status scan/open failure makes the entire status unavailable (`None`); it
  must never be rendered or acted on as a known-clean result.
- Main worktree info: `name: "main"` (literal), `is_main: true`.
- HEAD sha: short (7+) via `object.short_id()`.
- For missing dirs: read `git_dir/worktrees/<name>/HEAD` textually to recover
  the branch name ("ref: refs/heads/x") so `list` can still label it.

## src/gitcmd.rs — mutations (the ONLY place `git` is spawned)

```rust
use std::path::Path;
use crate::error::Result;

/// Run `git <args>` with cwd = `cwd`, capturing output. Non-zero exit ⇒
/// Error::GitCommand with captured stderr.
pub fn run(cwd: &Path, args: &[&str]) -> Result<()>;

/// Add an existing branch. Quiet captures Git output; otherwise it streams.
pub fn worktree_add(main_root: &Path, path: &Path, branch: &str, quiet: bool) -> Result<()>;
/// Create and add a branch. Quiet captures Git output; otherwise it streams.
pub fn worktree_add_new_branch(main_root: &Path, path: &Path, branch: &str, base: &str, quiet: bool) -> Result<()>;
/// `git worktree remove [--force] <path>`.
pub fn worktree_remove(main_root: &Path, path: &Path, force: bool) -> Result<()>;
/// `git worktree prune`.
pub fn worktree_prune(main_root: &Path) -> Result<()>;
/// `git branch -D <name>` (only ever called after explicit user opt-in).
pub fn branch_delete(main_root: &Path, name: &str) -> Result<()>;
```

## src/config.rs — layered configuration

```rust
use std::path::{Path, PathBuf};
use crate::error::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]  // on the *file* struct; merged Config is plain
pub struct ConfigFile { /* every field Option<...>, including nested */ }

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub path_template: String,        // default "../{repo}-worktrees/{branch}"
    pub default_base: Option<String>, // default Some("origin/main")? NO — default None means "use HEAD"; built-in default is None. Config may set e.g. "origin/main".
    pub editor: Option<String>,       // resolution order at use site: config > $VISUAL > $EDITOR
    pub setup: SetupConfig,
    pub prune: PruneConfig,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SetupConfig {
    pub commands: Vec<String>,
    pub copy: Vec<CopyEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct CopyEntry {
    pub path: String,
    #[serde(default)]
    pub mode: CopyMode, // Copy | Symlink; serde rename_all lowercase; default Copy
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CopyMode { #[default] Copy, Symlink }

#[derive(Debug, Clone, PartialEq)]
pub struct PruneConfig {
    pub protected_branches: Vec<String>, // default ["main","master","develop"]
}

impl Default for Config { /* built-in defaults above */ }

/// Layered load; later overrides earlier, field-by-field (a file that sets
/// only `editor` must not clobber an earlier file's `path_template`):
/// defaults < global (~/.config/wtm/config.toml via `directories`
/// ProjectDirs "wtm" — but PREFER the plain `~/.config/wtm/config.toml` path
/// on macOS too, matching the README; use env override WTM_CONFIG_DIR for
/// tests) < `<repo>/.worktree.toml` < `<repo>/.worktree.local.toml`.
/// `setup.copy` and `prune.protected_branches` replace earlier values.
/// Executable `editor` and `setup.commands` are rejected in shared
/// `.worktree.toml`; they are accepted only from global or local trusted
/// config. Unparseable or untrusted executable config ⇒ Error::Config with
/// the file path.
pub fn load(repo_root: &Path) -> Result<Config>;

/// Merge a parsed file over a config (exposed for unit tests).
pub fn merge(base: Config, layer: ConfigFile) -> Config;

/// Path of the global config file (honoring $WTM_CONFIG_DIR override).
pub fn global_config_path() -> Option<PathBuf>;

/// Write a fully commented sample `.worktree.toml` at repo root. Errors if it
/// already exists. Returns the path written.
pub fn scaffold_repo_config(repo_root: &Path) -> Result<PathBuf>;
```

On macOS use `~/.config/wtm/config.toml` (XDG-style), NOT Library/Application
Support — dev CLIs conventionally use ~/.config. Implement as:
`$WTM_CONFIG_DIR` > `$XDG_CONFIG_HOME/wtm` > `~/.config/wtm` (via
`directories::BaseDirs::home_dir()`).

## src/template.rs — path template rendering (NEW worktrees only)

```rust
use std::path::{Path, PathBuf};
use crate::error::Result;

pub struct TemplateContext<'a> {
    pub repo_name: &'a str,   // {repo}
    pub branch: &'a str,      // {branch} raw, may contain '/'
    pub main_root: &'a Path,  // {repo_dir}; also the base for relative results
}

/// Render placeholders {repo} {branch} {slug} {home} {repo_dir}. Unknown
/// {placeholder} ⇒ Error::Template. Relative results are joined to
/// `main_root` and lexically normalized (no filesystem access, handles ".."
/// and "."). Absolute results are normalized too.
pub fn render(template: &str, ctx: &TemplateContext) -> Result<PathBuf>;

/// Filesystem-safe branch slug: '/' and any char outside [A-Za-z0-9._-]
/// become '-', runs collapse, leading/trailing '-' trimmed, never empty
/// (fallback "branch"). Case preserved.
pub fn slugify(branch: &str) -> String;

/// Lexical normalization helper (pub for reuse/tests).
pub fn normalize(path: &Path) -> PathBuf;
```

## src/setup.rs — post-create automation

```rust
use std::path::Path;
use crate::config::Config;
use crate::error::Result;

/// Run post-create automation in this order:
/// 1. Validate each setup.copy path as non-empty, relative, and free of `..`,
///    root, or prefix components. Canonical sources must remain inside the
///    main worktree. Reject source symlinks, recursive symlinks, and symlinked
///    destination parents. Never overwrite an existing destination. Copy
///    regular files/directories or create an absolute symlink to a contained
///    regular source file.
/// 2. Run each setup.commands entry via `sh -c`, cwd = worktree, streaming
///    stdout/stderr to the user (inherit). First failing command ⇒
///    Error::Setup naming the command (the worktree stays).
pub fn run(config: &Config, main_root: &Path, worktree: &Path, quiet: bool) -> Result<()>;
```

## src/output.rs — rendering

```rust
use crate::model::WorktreeInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorMode { Auto, Always, Never }

/// True when color should be used on stdout: mode Always ⇒ true; Never ⇒
/// false; Auto ⇒ stdout is a TTY AND env NO_COLOR is unset/empty.
pub fn use_color(mode: ColorMode) -> bool;

/// Human table for `wtm list`. Columns: NAME (branch or registry name, main
/// marked), PATH (with ~ abbreviation), HEAD, AHEAD/BEHIND ("↑2 ↓1", "-"
/// when no upstream, "gone" when upstream_gone), STATUS badges
/// (dirty/merged/missing/locked/prunable, colored when enabled). When status
/// was skipped, omit the status-derived columns.
pub fn render_table(items: &[WorktreeInfo], color: bool, with_status: bool) -> String;

/// `--json`: serde_json pretty array of WorktreeInfo, stable field names as
/// declared in model.rs.
pub fn render_json(items: &[WorktreeInfo]) -> String;
```

## src/cli.rs — clap derive definitions

```rust
#[derive(clap::Parser)]
#[command(name = "wtm", version, about, propagate_version = true)]
pub struct Cli {
    #[command(subcommand)] pub command: Command,
    #[command(flatten)] pub global: GlobalArgs,
}

#[derive(clap::Args, Debug, Clone)]
pub struct GlobalArgs {
    /// Operate on the repository at this path instead of the cwd.
    #[arg(short = 'C', long = "repo", global = true, value_name = "PATH")]
    pub repo: Option<std::path::PathBuf>,
    #[arg(long, global = true, value_enum, default_value_t = ColorMode::Auto)]
    pub color: ColorMode,
    #[arg(short, long, global = true)] pub verbose: bool,
    #[arg(short, long, global = true, conflicts_with = "verbose")] pub quiet: bool,
}

pub enum Command {
    Add(AddArgs),        // aliases: new, create
    List(ListArgs),      // alias: ls; --json; --no-status alias --fast
    Remove(RemoveArgs),  // alias: rm; --force, --with-branch
    Switch(SwitchArgs),  // aliases: cd, sw; hidden --print-path
    Prune(PruneArgs),    // alias: clean; --merged --gone --dry-run --force
    Open(OpenArgs),      // --with <cmd>
    Path(PathArgs),
    Init(InitArgs),      // shell: zsh|bash (ValueEnum Shell)
    Completions(CompletionsArgs),
    Config(ConfigArgs),  // subcommands: path, init
}
```
AddArgs: `branch: String`, `--from <base>`, `--path <path>`, `--cd`, `--open`,
`--no-setup`. `--json` lives ONLY on read commands (list; path/switch emit
plain text). Every command and flag gets real help text (doc comments).

## src/commands/ — one module per command

`pub mod add; pub mod completions; pub mod config_cmd; pub mod init;
pub mod list; pub mod open; pub mod path; pub mod prune; pub mod remove;
pub mod switch;` — each exposes
`pub fn run(args: &XArgs, global: &GlobalArgs) -> crate::error::Result<()>`
(signature may take resolved `RepoContext`/`Config` instead — keep it
consistent across all commands: resolve ctx+config in a shared
`commands::prepare(global)` helper).

Key behaviors:
- add: branch exists (local) ⇒ error BranchInUse if some worktree already has
  it checked out, else `worktree_add`. Branch doesn't exist ⇒ base = --from >
  config.default_base > HEAD. Any explicitly selected base must resolve and
  peel to a commit; otherwise fail before mutation. Then call
  `worktree_add_new_branch`. Destination = --path >
  template::render; refuse if destination exists (DestinationExists). Then
  setup (unless --no-setup); on Error::Setup print it but exit non-zero.
  --open: preflight and launch the editor on the new path. Print success only
  outside quiet mode. `--cd` writes the target only after setup and all other
  requested post-create actions succeed.
- list: with_status = !no_status; --json ⇒ render_json to stdout.
- remove: name optional ⇒ interactive picker (TTY-gated, see picker rules).
  Refuse main worktree (MainWorktree). Refuse when target contains cwd.
  Safety: if dirty and !force ⇒ Error::Dirty. Missing dir ⇒ remove registry
  entry via `git worktree remove --force` (it's the only way) but only ever
  after informing the user via stderr note; still safe. --with-branch ⇒
  branch_delete after successful removal, but refuse for protected branches.
- switch: resolve worktree; with --print-path (hidden flag) print ONLY the
  path to stdout (ALL other UI, including the picker, must go to stderr);
  without it print the path plus a hint (stderr) about `wtm init zsh`.
- prune: candidates = missing/prunable entries (always) + merged (only with
  --merged) + upstream_gone (only with --gone). Skip main worktree and any
  candidate whose branch ∈ protected_branches. --dry-run prints the plan and
  exits 0. Respect dirty-safety like remove unless --force. Always finish
  with `git worktree prune`. Process candidates independently, continue after
  removal or branch-deletion failures, and report failures together after the
  registry refresh. Branch deletion: merged/gone candidates get
  their branch deleted (that is the point of pruning); protected branches
  never; missing-dir entries never (we only clean the registry).
- open: resolve worktree (picker if omitted); `--with <cmd>` ⇒ run via
  `sh -c` with cwd = worktree, wait, propagate failure; else editor =
  config.editor > $VISUAL > $EDITOR (error Config if none set). Preflight the
  executable before reporting success, then spawn detached with the path as
  an argument.
- path: resolve worktree, print path. Never a picker (scripting-friendly):
  if name is omitted, discover the nearest Git worktree directly without
  paying for a full registry listing. An explicit `-C` instead scopes
  containment to that repository's registry.
- init: print the shell function + `eval` of completions for zsh or bash to
  stdout (see wrapper below).
- completions: clap_complete::generate to stdout.
- config path: print global path and (if in a repo) repo-level paths with
  existence markers. config init: scaffold_repo_config.

### Interactive picker rules (single helper, e.g. commands::pick)
- Allowed ONLY when stdin AND stderr are TTYs (`IsTerminal`); otherwise
  Error::NotATty. stdout deliberately NOT required to be a TTY (the shell
  wrapper captures stdout).
- inquire renders on stdout by default. To keep captured stdout clean, wrap
  the prompt in an fd-swap guard: `libc::dup(1)` to save stdout, `dup2(2, 1)`
  before prompting, restore + close after (RAII guard so errors restore too).
  Verify empirically in an integration test that `--print-path` stdout
  contains only the path.
- Use inquire::Select with the worktree display names, fuzzy filter enabled.

### Shell wrapper (`wtm init zsh|bash` output; identical for both plus
completions differ)
```sh
wtm() {
  local cdfile; cdfile="$(mktemp -t wtm-cd.XXXXXX)" || return
  WTM_CD_FILE="$cdfile" command wtm "$@"; local status=$?
  if [ "$status" -eq 0 ] && [ -s "$cdfile" ]; then
    local target; target="$(cat "$cdfile")"; target="${target%.}"
    builtin cd -- "$target" || status=$?
  fi
  rm -f "$cdfile"; return $status
}
```
The binary only accepts a private, regular, non-symlink file inside the
system temp directory with the `wtm-cd.` prefix. The trailing sentinel keeps
paths with newlines and non-UTF-8 bytes intact on Unix.
zsh completions: `eval "$(command wtm completions zsh)"` needs compdef; emit
the standard pattern (autoload -Uz compinit guard comment + source the
completion script via a temp file or `eval`). bash: `eval "$(command wtm
completions bash)"`.

## src/main.rs — thin entrypoint

Parse Cli, dispatch to commands::run(cli), map Err to stderr message
(`error: {err:#}` styled red when stderr color allowed) and exit code 1
(clap handles usage errors with exit 2 itself). Interactive picker
cancellation is a silent success. No other logic.

## Performance rules
- Never spawn `git` on a read path.
- `list --no-status` must do no per-worktree repository opens beyond reading
  registry metadata (plus the cheap HEAD/branch reads).
- Status via rayon par_iter, one git2 Repository open per worktree.
- Keep startup lazy: config/template loading only for commands that need it.
- The ignored release-mode gate builds 64 linked worktrees (65 including
  main), measures first-load latency against 1 second, and measures the median
  of 11 warm loads against 500ms.

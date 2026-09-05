//! The GUI's blocking data layer: git/worktree queries via the `wtm` library
//! and `git2`, plus the platform glue (clipboard, terminal, reveal, open URL,
//! fetch) the app needs off the UI thread.
//!
//! Every function here is blocking, so views call them through
//! [`gpui::AppContext::background_spawn`] and apply the result back on the
//! foreground.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use wtm::commands::{add, open, prune, remove};
use wtm::config::{self, Config};
use wtm::gitcmd;
use wtm::model::WorktreeInfo;
use wtm::repo::{self, RepoContext};
use wtm::setup::SetupEvent;
use wtm::worktree::{self, ListOptions, WorktreeDetails};

/// A repository the app has open: its resolved context plus the merged
/// configuration for it. Both are cheap value types, so this is cloned into
/// background tasks freely.
#[derive(Clone)]
pub struct OpenRepo {
    pub ctx: RepoContext,
    pub config: Config,
}

impl OpenRepo {
    /// Display name of the repository (its main working tree's directory).
    pub fn name(&self) -> &str {
        &self.ctx.repo_name
    }

    /// Absolute path of the main working tree.
    pub fn path(&self) -> &Path {
        &self.ctx.main_root
    }
}

/// Resolve `path` to a repository and load its configuration.
///
/// `path` may point anywhere inside the repo — including a linked worktree or
/// a subdirectory — and still resolves to the main working tree, which is what
/// the registry keys on.
pub fn open_repo(path: &Path) -> Result<OpenRepo, String> {
    let ctx = repo::discover(Some(path)).map_err(|e| e.to_string())?;
    let config = config::load(&ctx.main_root).map_err(|e| e.to_string())?;
    Ok(OpenRepo { ctx, config })
}

/// Resolve the repository containing the current working directory, if any.
/// Used when the app is launched from a terminal inside a repo.
pub fn open_repo_from_cwd() -> Option<OpenRepo> {
    let cwd = std::env::current_dir().ok()?;
    open_repo(&cwd).ok()
}

/// List worktrees for a repository.
///
/// `with_status` drives the expensive part (dirty/ahead/behind/merged). The
/// app loads without status first so the table paints immediately, then loads
/// again with status — the same two-pass strategy the TUI uses.
pub fn list_worktrees(repo: &OpenRepo, with_status: bool) -> Result<Vec<WorktreeInfo>, String> {
    let base = if with_status {
        repo.config.default_base.clone()
    } else {
        None
    };
    worktree::list(&repo.ctx, &ListOptions { with_status, base }).map_err(|e| e.to_string())
}

/// Detail-pane data (recent commits, upstream, remote) for one worktree.
pub fn worktree_details(path: &Path) -> Option<WorktreeDetails> {
    worktree::details(path)
}

/// Create a worktree for `branch`, reporting post-create setup progress
/// through `sink` as it happens instead of running it silently — for a
/// create-dialog progress view that has to show something during a slow
/// `npm install` rather than freezing until it finishes.
pub fn create_worktree_streaming(
    repo: &OpenRepo,
    branch: &str,
    base: Option<&str>,
    run_setup: bool,
    sink: &mut dyn FnMut(SetupEvent),
) -> Result<PathBuf, String> {
    let request = add::CreateRequest {
        branch,
        base_override: base,
        path_override: None,
        cd: false,
        run_setup,
        announce: false,
        quiet: true,
        verbose: false,
    };
    add::create_streaming(&repo.ctx, &repo.config, &request, sink).map_err(|e| e.to_string())
}

/// Remove a worktree through the safety-checked core (main worktree, cwd, and
/// dirty checks all still apply).
pub fn remove_worktree(repo: &OpenRepo, info: &WorktreeInfo, force: bool) -> Result<(), String> {
    remove::remove_worktree(&repo.ctx, info, force, true).map_err(|e| e.to_string())
}

/// Which worktrees `prune` would sweep, given the repo's protected branches.
pub fn prune_candidates(
    repo: &OpenRepo,
    rows: &[WorktreeInfo],
    merged: bool,
    gone: bool,
) -> Vec<prune::PruneCandidate> {
    prune::candidates(
        rows,
        &repo.config.prune.protected_branches,
        merged,
        gone,
        false,
    )
}

/// Prune candidates the user has explicitly selected in the table.
pub fn selection_candidates(
    repo: &OpenRepo,
    rows: Vec<WorktreeInfo>,
) -> Vec<prune::PruneCandidate> {
    prune::selection_candidates(rows, &repo.config.prune.protected_branches)
}

/// Execute a prune over already-confirmed candidates.
pub fn run_prune(
    repo: &OpenRepo,
    candidates: &[prune::PruneCandidate],
    force: bool,
) -> prune::PruneReport {
    prune::execute(&repo.ctx, candidates, force, false)
}

/// Open a worktree in the configured editor (config `editor` > `$VISUAL` >
/// `$EDITOR`). This is what activating a row in the app does.
pub fn open_in_editor(repo: &OpenRepo, path: &Path) -> Result<(), String> {
    open::spawn_editor(&repo.config, path).map_err(|e| e.to_string())
}

/// Reveal a path in the platform's file manager: Finder on macOS via
/// `open -R`, or the freedesktop D-Bus "FileManager1" interface on Linux
/// (falling back to `xdg-open` on its containing directory). The Settings
/// sheet's config files are often not created yet, so a missing `path` walks
/// up to the nearest existing ancestor directory instead of failing outright.
pub fn reveal_in_finder(path: &Path) -> Result<(), String> {
    let Some(target) = existing_ancestor(path) else {
        // Only possible for a relative path with no existing prefix at all
        // (an absolute path always bottoms out at a filesystem root).
        return Err(format!(
            "cannot reveal '{}': neither it nor any parent directory exists",
            path.display()
        ));
    };
    let redirected = target.as_path() != path;

    reveal_target(&target).map_err(|e| {
        if redirected {
            format!(
                "'{}' does not exist yet; revealing '{}' instead also failed: {e}",
                path.display(),
                target.display()
            )
        } else {
            e
        }
    })
}

#[cfg(target_os = "macos")]
fn reveal_target(target: &Path) -> Result<(), String> {
    std::process::Command::new("open")
        .arg("-R")
        .arg(target)
        .status()
        .map_err(|e| format!("could not launch Finder: {e}"))
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(format!("Finder exited with {status}"))
            }
        })
}

/// Linux: try the D-Bus `FileManager1.ShowItems` call first -- it can select
/// `target` itself, not just open its directory -- and only fall back to
/// `xdg-open` when that fails outright, so a working file manager still gets
/// the more precise result.
#[cfg(not(target_os = "macos"))]
fn reveal_target(target: &Path) -> Result<(), String> {
    let Err(dbus_err) = reveal_via_dbus(target) else {
        return Ok(());
    };
    let dir = xdg_open_fallback_target(target);
    std::process::Command::new("xdg-open")
        .arg(dir)
        .status()
        .map_err(|e| {
            format!("D-Bus FileManager1 failed ({dbus_err}); xdg-open could not be launched either: {e}")
        })
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(format!(
                    "D-Bus FileManager1 failed ({dbus_err}); xdg-open exited with {status}"
                ))
            }
        })
}

/// Ask whichever file manager registered `org.freedesktop.FileManager1` on
/// the session bus to open its window on `target`'s containing folder with
/// `target` itself selected. Implemented via the `dbus-send` CLI rather than
/// pulling in a D-Bus client library -- this is the one D-Bus call in the
/// whole app, not worth a new dependency for.
#[cfg(not(target_os = "macos"))]
fn reveal_via_dbus(target: &Path) -> Result<(), String> {
    let uri = format!("file://{}", target.display());
    std::process::Command::new("dbus-send")
        .args([
            "--session",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
            &format!("array:string:{uri}"),
            "string:",
        ])
        .status()
        .map_err(|e| format!("could not launch dbus-send: {e}"))
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(format!("dbus-send exited with {status}"))
            }
        })
}

/// The directory `xdg-open` should be pointed at as the reveal fallback:
/// `target` itself if it's a directory, otherwise its parent -- `xdg-open`
/// has no notion of "open this folder with this file selected". Split out of
/// `reveal_target` so the directory-vs-parent choice is unit tested without
/// spawning `xdg-open`.
#[cfg(not(target_os = "macos"))]
fn xdg_open_fallback_target(target: &Path) -> &Path {
    if target.is_dir() {
        target
    } else {
        target.parent().unwrap_or(target)
    }
}

/// The nearest existing ancestor of `path`: `path` itself if it already
/// exists, otherwise the first parent directory that does. `None` only when
/// no ancestor exists either. Pure path logic split out of
/// `reveal_in_finder` so it can be unit tested without spawning Finder.
fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return Some(path.to_path_buf());
    }
    path.ancestors()
        .skip(1)
        .find(|p| p.exists())
        .map(Path::to_path_buf)
}

/// Open a worktree in a terminal app.
///
/// macOS: `$WTM_TERMINAL` names the app for `open -a <app> <path>`, falling
/// back to `Terminal`.
///
/// Linux: `$WTM_TERMINAL` names the emulator binary to try first (a bare
/// name resolved on `$PATH`, or a full path); failing that, the first
/// installed of, in order, `x-terminal-emulator`, `gnome-terminal`,
/// `konsole`, `alacritty`, `kitty`, `wezterm`, `foot`, `xterm`. Each is
/// spawned detached (`Command::spawn`, never waited on) rather than launched
/// the way macOS's `open -a` is: `open` itself exits the moment the app is
/// launched, but several of these terminals (xterm, alacritty, kitty, foot,
/// wezterm) run in the foreground and don't return control until their
/// window closes, so waiting on them here would block for as long as the
/// user keeps the terminal open.
pub fn open_in_terminal(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let terminal = std::env::var("WTM_TERMINAL")
            .ok()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "Terminal".to_string());
        std::process::Command::new("open")
            .arg("-a")
            .arg(&terminal)
            .arg(path)
            .status()
            .map_err(|e| format!("could not launch {terminal}: {e}"))
            .and_then(|status| {
                if status.success() {
                    Ok(())
                } else {
                    Err(format!("{terminal} exited with {status}"))
                }
            })
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Ok(explicit) = std::env::var("WTM_TERMINAL") {
            if !explicit.is_empty() {
                return spawn_terminal(&explicit, path);
            }
        }
        const CANDIDATES: &[&str] = &[
            "x-terminal-emulator",
            "gnome-terminal",
            "konsole",
            "alacritty",
            "kitty",
            "wezterm",
            "foot",
            "xterm",
        ];
        for name in CANDIDATES {
            if spawn_terminal(name, path).is_ok() {
                return Ok(());
            }
        }
        Err(format!(
            "no terminal emulator found (tried: {})",
            CANDIDATES.join(", ")
        ))
    }
}

/// Launch one Linux terminal candidate, detached. `program` may be a bare
/// name (`"gnome-terminal"`, looked up on `$PATH`) or a full path (from
/// `$WTM_TERMINAL`); either way its file name is what [`terminal_args`]
/// matches against to decide which working-directory flag it needs. `Err`
/// covers both "not installed" (spawn failed) and any other spawn failure --
/// both mean the caller should try the next candidate rather than stop.
#[cfg(not(target_os = "macos"))]
fn spawn_terminal(program: &str, path: &Path) -> Result<(), String> {
    let match_name = Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program);
    std::process::Command::new(program)
        // Set unconditionally: load-bearing for the terminals that inherit
        // cwd from the spawning process (see `terminal_args`), harmless for
        // the ones that need an explicit flag instead.
        .current_dir(path)
        .args(terminal_args(match_name, path))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not launch {program}: {e}"))
}

/// Extra CLI args that make `program` open with `path` as its working
/// directory, keyed on the emulator's executable name. Terminals differ here
/// in a way that can't be papered over with one flag: some read the
/// *spawning process's* cwd directly (xterm, and `x-terminal-emulator` --
/// itself a Debian alternatives symlink to an unknown underlying terminal,
/// so relying on inherited cwd is the only thing that works for every
/// possible target); others (gnome-terminal) talk to a persistent daemon
/// over D-Bus whose own cwd is what a new window inherits by default,
/// regardless of the client process's cwd, so those need an explicit flag.
/// Pure and unit tested directly -- no process is spawned to test this.
#[cfg(not(target_os = "macos"))]
fn terminal_args(program: &str, path: &Path) -> Vec<std::ffi::OsString> {
    let path = path.as_os_str();
    match program {
        "gnome-terminal" | "foot" => {
            let mut arg = std::ffi::OsString::from("--working-directory=");
            arg.push(path);
            vec![arg]
        }
        "konsole" => vec!["--workdir".into(), path.to_os_string()],
        "alacritty" => vec!["--working-directory".into(), path.to_os_string()],
        "kitty" => vec!["--directory".into(), path.to_os_string()],
        "wezterm" => vec!["start".into(), "--cwd".into(), path.to_os_string()],
        // xterm and x-terminal-emulator: no flag, rely on the inherited cwd
        // set by `Command::current_dir` in `spawn_terminal`.
        _ => Vec::new(),
    }
}

/// Copy `text` to the system clipboard.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    wtm::clipboard::copy(text)
        .map(drop)
        .map_err(|e| e.to_string())
}

/// One branch as shown in the create-worktree dialog's branch picker.
#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub name: String,
    /// `dialogs.rs` constructs `BranchInfo` values with this field but only
    /// ever reads `name`/`is_checked_out`/`upstream_gone` back.
    #[allow(dead_code)]
    pub is_local: bool,
    /// Already checked out in some worktree of this repository (a `wtm add`
    /// for it would fail with `BranchInUse`).
    pub is_checked_out: bool,
    /// Local branches only: has upstream configuration but the upstream ref
    /// is missing (same semantics as `WorktreeStatus::upstream_gone`).
    pub upstream_gone: bool,
}

/// Branches available to create a worktree from: local branches first
/// (alphabetical), then remote-tracking branches (alphabetical, remote
/// prefix stripped, `<remote>/HEAD` excluded, and any name already covered
/// by a local branch or another remote removed).
pub fn list_branches(repo: &OpenRepo) -> Result<Vec<BranchInfo>, String> {
    let git_repo = repo.ctx.open_main().map_err(|e| e.to_string())?;

    // Branches checked out in some worktree right now, so the picker can
    // flag entries that `wtm add` would refuse with `BranchInUse`.
    let checked_out: HashSet<String> = worktree::list(
        &repo.ctx,
        &ListOptions {
            with_status: false,
            base: None,
        },
    )
    .map_err(|e| e.to_string())?
    .into_iter()
    .filter_map(|info| info.branch)
    .collect();

    let mut local_names: HashSet<String> = HashSet::new();
    let mut locals: Vec<BranchInfo> = Vec::new();
    let local_branches = git_repo
        .branches(Some(git2::BranchType::Local))
        .map_err(|e| e.to_string())?;
    for entry in local_branches {
        let (branch, _) = entry.map_err(|e| e.to_string())?;
        let Some(name) = branch.name().map_err(|e| e.to_string())?.map(str::to_owned) else {
            continue; // Not a valid UTF-8 ref name; nothing sane to show.
        };
        let upstream_gone = worktree::upstream_gone(&git_repo, &branch);
        local_names.insert(name.clone());
        locals.push(BranchInfo {
            is_checked_out: checked_out.contains(&name),
            name,
            is_local: true,
            upstream_gone,
        });
    }
    locals.sort_by(|a, b| a.name.cmp(&b.name));

    let mut remotes: Vec<BranchInfo> = Vec::new();
    let remote_branches = git_repo
        .branches(Some(git2::BranchType::Remote))
        .map_err(|e| e.to_string())?;
    for entry in remote_branches {
        let (branch, _) = entry.map_err(|e| e.to_string())?;
        let Some(full_name) = branch.name().map_err(|e| e.to_string())?.map(str::to_owned) else {
            continue;
        };
        let Some((_remote, short)) = full_name.split_once('/') else {
            continue; // Not `<remote>/<name>` shaped; skip rather than guess.
        };
        if short == "HEAD" {
            continue; // The remote's symbolic HEAD pointer, not a real branch.
        }
        if local_names.contains(short) {
            continue; // Already represented by its local branch.
        }
        remotes.push(BranchInfo {
            name: short.to_string(),
            is_local: false,
            is_checked_out: checked_out.contains(short),
            upstream_gone: false,
        });
    }
    remotes.sort_by(|a, b| a.name.cmp(&b.name));
    // Two remotes tracking the same branch name (e.g. origin/main and
    // upstream/main) collapse to one entry now that the remote prefix is
    // gone.
    remotes.dedup_by(|a, b| a.name == b.name);

    locals.extend(remotes);
    Ok(locals)
}

/// Delete a local branch, refusing the repo's protected branches (mirrors
/// `wtm prune`'s and `wtm rm --with-branch`'s safety rule).
pub fn delete_branch(repo: &OpenRepo, branch: &str) -> Result<(), String> {
    if repo
        .config
        .prune
        .protected_branches
        .iter()
        .any(|p| p == branch)
    {
        return Err(format!(
            "branch '{branch}' is protected and will not be touched"
        ));
    }
    gitcmd::branch_delete(&repo.ctx.main_root, branch).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------
// Base-ref picker (new-worktree dialog)
// ---------------------------------------------------------------------

/// What a [`RefInfo`] represents, so a base-ref picker can label it instead
/// of showing a flat list of names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefKind {
    /// Checked out in the worktree the user is currently looking at.
    Current,
    /// The repo's configured default base (`config.default_base`), or HEAD
    /// when unset — what `wtm add` uses when the caller doesn't override the
    /// base (see `commands::add::resolve_base`).
    Default,
    /// Checked out in some other worktree — `wtm add` would refuse it.
    Worktree,
    /// An ordinary local branch.
    Local,
    /// A remote-tracking branch; `remote` names which remote.
    Remote { remote: String },
}

/// One ref as shown in the create-worktree dialog's base-ref picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefInfo {
    /// What to pass to git as the base (e.g. `main`, `origin/feature/x`).
    pub name: String,
    /// What to show in the picker (may equal `name`).
    pub display: String,
    pub kind: RefKind,
    /// Commit subject + short sha for a secondary line, if cheap to get.
    pub subject: Option<String>,
    pub short_id: Option<String>,
}

/// Refs available to branch a new worktree from, for the create-worktree
/// dialog's base-ref picker: local branches, remote-tracking branches, and
/// two synthetic markers (`Current`, `Default`) called out on whichever
/// entries carry that meaning. Unlike [`list_branches`], a local branch and
/// its remote-tracking counterpart are kept as separate entries — branching
/// from `origin/main` vs local `main` is a real distinction here.
///
/// `current_worktree` is the worktree the dialog was opened from, if known;
/// its checked-out branch (if any) becomes the `Current` entry.
pub fn list_refs(repo: &OpenRepo, current_worktree: Option<&Path>) -> Result<Vec<RefInfo>, String> {
    let git_repo = repo.ctx.open_main().map_err(|e| e.to_string())?;

    // One `worktree::list` call tells us both which branch is checked out
    // in `current_worktree` and which branches are checked out in some
    // OTHER worktree (`wtm add` would refuse those — `RefKind::Worktree`).
    let worktrees = worktree::list(
        &repo.ctx,
        &ListOptions {
            with_status: false,
            base: None,
        },
    )
    .map_err(|e| e.to_string())?;
    let current_canon = current_worktree.map(repo::canonicalize_lossy);
    let mut current_branch: Option<String> = None;
    let mut checked_out_elsewhere: HashSet<String> = HashSet::new();
    for w in &worktrees {
        if current_canon.as_deref() == Some(w.path.as_path()) {
            current_branch = w.branch.clone();
        } else if let Some(name) = &w.branch {
            checked_out_elsewhere.insert(name.clone());
        }
    }

    let mut refs: Vec<RawRef> = Vec::new();
    for entry in git_repo
        .branches(Some(git2::BranchType::Local))
        .map_err(|e| e.to_string())?
    {
        let (branch, _) = entry.map_err(|e| e.to_string())?;
        let Some(name) = branch.name().map_err(|e| e.to_string())?.map(str::to_owned) else {
            continue; // Not a valid UTF-8 ref name; nothing sane to show.
        };
        refs.push(RawRef { name, remote: None });
    }
    for entry in git_repo
        .branches(Some(git2::BranchType::Remote))
        .map_err(|e| e.to_string())?
    {
        let (branch, _) = entry.map_err(|e| e.to_string())?;
        let Some(full_name) = branch.name().map_err(|e| e.to_string())?.map(str::to_owned) else {
            continue;
        };
        let Some((remote, short)) = full_name.split_once('/') else {
            continue; // Not `<remote>/<name>` shaped; skip rather than guess.
        };
        if short == "HEAD" {
            continue; // The remote's symbolic HEAD pointer, not a real branch.
        }
        refs.push(RawRef {
            name: format!("{remote}/{short}"),
            remote: Some(remote.to_string()),
        });
    }

    let default_base = repo.config.default_base.as_deref();
    let mut ordered = assemble_ref_order(
        refs,
        current_branch.as_deref(),
        &checked_out_elsewhere,
        default_base,
    );

    // Subject + short id are best-effort dressing, not identity: a ref that
    // fails to resolve (e.g. the synthetic "HEAD" entry on a repo with no
    // commits yet) still keeps its place in the list.
    for r in &mut ordered {
        let (subject, short_id) = commit_info(&git_repo, &r.name);
        r.subject = subject;
        r.short_id = short_id;
    }

    Ok(ordered)
}

/// One candidate ref gathered from git, before kind-assignment and ordering.
/// Deliberately doesn't touch git2 so [`assemble_ref_order`] can be unit
/// tested without a real repository.
struct RawRef {
    /// Local: the branch name. Remote: `<remote>/<short>`.
    name: String,
    /// `None` for a local branch, `Some(remote)` for a remote-tracking one.
    remote: Option<String>,
}

/// A `RefInfo` with only `name`/`display`/`kind` filled in — what
/// [`assemble_ref_order`] can produce without touching git.
fn bare_ref(name: String, kind: RefKind) -> RefInfo {
    RefInfo {
        display: name.clone(),
        name,
        kind,
        subject: None,
        short_id: None,
    }
}

/// Order and kind-assign a flat set of refs into picker order: Current
/// first, then Default, then other local branches (alphabetical), then
/// remote-tracking branches (alphabetical).
///
/// A ref is Current when it matches `current_branch`; that check runs
/// before the Default check, so a branch that is both the current one AND
/// the configured default becomes a single `Current` entry rather than
/// showing up twice. Only once nothing local/remote matches `default_base`
/// (or `current_branch` already claimed it) does a synthetic Default entry
/// get created — this is the common case, since an unset `default_base`
/// resolves to the literal name `"HEAD"`, which is never itself a branch.
///
/// Pure: no git access, so this is unit tested directly. [`list_refs`] is
/// the git-facing wrapper that gathers `refs`, resolves `subject`/`short_id`
/// per entry, and calls this.
fn assemble_ref_order(
    refs: Vec<RawRef>,
    current_branch: Option<&str>,
    checked_out_elsewhere: &HashSet<String>,
    default_base: Option<&str>,
) -> Vec<RefInfo> {
    let default_name = default_base.unwrap_or("HEAD");

    let mut current_entry: Option<RefInfo> = None;
    let mut default_entry: Option<RefInfo> = None;
    let mut other_locals: Vec<RefInfo> = Vec::new();
    let mut other_remotes: Vec<RefInfo> = Vec::new();

    for r in refs {
        match r.remote {
            None if current_branch == Some(r.name.as_str()) => {
                current_entry = Some(bare_ref(r.name, RefKind::Current));
            }
            None if default_entry.is_none() && r.name == default_name => {
                default_entry = Some(bare_ref(r.name, RefKind::Default));
            }
            None => {
                let kind = if checked_out_elsewhere.contains(&r.name) {
                    RefKind::Worktree
                } else {
                    RefKind::Local
                };
                other_locals.push(bare_ref(r.name, kind));
            }
            Some(_) if default_entry.is_none() && r.name == default_name => {
                default_entry = Some(bare_ref(r.name, RefKind::Default));
            }
            Some(remote) => {
                other_remotes.push(bare_ref(r.name, RefKind::Remote { remote }));
            }
        }
    }

    // Nothing matched an existing ref — synthesize the Default entry,
    // unless it would just duplicate the Current row (see doc comment).
    let current_is_default = current_entry
        .as_ref()
        .is_some_and(|c| c.name == default_name);
    if default_entry.is_none() && !current_is_default {
        default_entry = Some(bare_ref(default_name.to_string(), RefKind::Default));
    }

    other_locals.sort_by(|a, b| a.name.cmp(&b.name));
    other_remotes.sort_by(|a, b| a.name.cmp(&b.name));

    let mut result = Vec::with_capacity(other_locals.len() + other_remotes.len() + 2);
    result.extend(current_entry);
    result.extend(default_entry);
    result.extend(other_locals);
    result.extend(other_remotes);
    result
}

/// Best-effort commit subject + short id for `refname`, resolved via
/// `revparse_single`. `(None, None)` for anything that doesn't resolve to a
/// commit (unborn HEAD, a stale ref, ...) rather than failing the listing —
/// these fields are secondary-line dressing for the picker, not identity.
fn commit_info(repo: &git2::Repository, refname: &str) -> (Option<String>, Option<String>) {
    let Ok(obj) = repo.revparse_single(refname) else {
        return (None, None);
    };
    let Ok(commit) = obj.peel_to_commit() else {
        return (None, None);
    };
    let subject = commit.summary().ok().flatten().map(str::to_owned);
    let short_id = worktree::short_id(repo, commit.id());
    (subject, short_id)
}

// ---------------------------------------------------------------------
// File browser
// ---------------------------------------------------------------------

/// Git status for one path in [`list_files`]/[`worktree_diff`], collapsing
/// git2's much larger `Status`/`Delta` bitflags down to what a file browser
/// or diff viewer needs to badge an entry with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

/// One entry in a [`list_files`] directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    /// Path relative to the worktree root.
    pub rel_path: PathBuf,
    pub is_dir: bool,
    /// Git status for this path, if any: modified / added / deleted /
    /// untracked / conflicted.
    pub status: Option<FileStatus>,
}

/// List one directory level inside `worktree` (not recursive — a file
/// browser expands lazily; walking the whole tree up front is exactly what
/// makes that slow on a big repo).
pub fn list_files(worktree: &Path, rel_dir: &Path) -> Result<Vec<FileEntry>, String> {
    if escapes_worktree(rel_dir) {
        return Err(format!("'{}' escapes the worktree", rel_dir.display()));
    }

    let repo = git2::Repository::open(worktree).map_err(|e| e.to_string())?;
    let dir = worktree.join(rel_dir);

    // One `statuses` call for the whole worktree, indexed by path below —
    // not one call per file, which would be O(n) status walks for an
    // n-entry directory.
    let mut status_opts = git2::StatusOptions::new();
    status_opts
        .include_untracked(true)
        // A wholly-new directory then shows up as ONE status entry (path
        // ending in `/`) instead of one per file inside it, which is
        // exactly the granularity a one-level-at-a-time browser wants (we
        // don't display those nested files until the user expands the
        // directory) and is cheaper besides.
        .recurse_untracked_dirs(false)
        .include_ignored(false)
        .exclude_submodules(true);
    let statuses = repo
        .statuses(Some(&mut status_opts))
        .map_err(|e| e.to_string())?;
    let status_by_path: HashMap<String, FileStatus> = statuses
        .iter()
        .filter_map(|e| {
            let path = e.path().ok()?.to_string();
            let status = file_status_from_git(e.status())?;
            Some((path, status))
        })
        .collect();

    let read_dir =
        std::fs::read_dir(&dir).map_err(|e| format!("could not list '{}': {e}", dir.display()))?;

    let mut entries = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        let is_dir = file_type.is_dir();
        let rel_path = rel_dir.join(&name);

        // Respect .gitignore: an ignored `node_modules`/`target` full of
        // noise would otherwise make the browser useless. libgit2 matches
        // directory-only ignore patterns (e.g. `target/`) against a path
        // that ends in a slash, so directories are probed with one added.
        let mut ignore_probe = rel_path.as_os_str().to_os_string();
        if is_dir {
            ignore_probe.push("/");
        }
        if repo
            .status_should_ignore(Path::new(&ignore_probe))
            .unwrap_or(false)
        {
            continue;
        }

        let status = status_by_path
            .get(rel_path.to_string_lossy().as_ref())
            .copied();

        entries.push(FileEntry {
            name,
            rel_path,
            is_dir,
            status,
        });
    }

    sort_file_entries(&mut entries);
    Ok(entries)
}

/// Rejects a relative path that could climb out of the worktree via a `..`
/// component. `list_files`/`file_diff` join this straight onto the worktree
/// root, so letting `..` through would let a caller read/diff anything on
/// disk, not just inside the worktree — rejected regardless of whether a
/// `..` would net out back inside, since that's simpler to reason about
/// (and get right) than resolving the path first.
fn escapes_worktree(rel: &Path) -> bool {
    rel.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// Directories first, then files, each alphabetical case-insensitively.
/// Pure sort logic pulled out of `list_files` so it can be unit tested
/// without touching git or the filesystem.
fn sort_file_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        (!a.is_dir, a.name.to_lowercase()).cmp(&(!b.is_dir, b.name.to_lowercase()))
    });
}

/// Collapse git2's per-side (index vs. working tree) status bits into one
/// [`FileStatus`]. Priority: a conflict always wins (it blocks everything
/// else); an untracked working-tree file is next (it has no index side to
/// also be "modified"); then renamed/deleted/added; anything left still
/// carrying a modified/typechange bit falls back to `Modified`. `None` when
/// none of the bits above are set (e.g. the entry is only ignored, which
/// `list_files`/`worktree_diff` already exclude upstream of this).
fn file_status_from_git(status: git2::Status) -> Option<FileStatus> {
    if status.is_conflicted() {
        return Some(FileStatus::Conflicted);
    }
    if status.is_wt_new() {
        return Some(FileStatus::Untracked);
    }
    if status.is_index_renamed() || status.is_wt_renamed() {
        return Some(FileStatus::Renamed);
    }
    if status.is_index_deleted() || status.is_wt_deleted() {
        return Some(FileStatus::Deleted);
    }
    if status.is_index_new() {
        return Some(FileStatus::Added);
    }
    if status.is_index_modified()
        || status.is_wt_modified()
        || status.is_index_typechange()
        || status.is_wt_typechange()
    {
        return Some(FileStatus::Modified);
    }
    None
}

// ---------------------------------------------------------------------
// Diff viewer
// ---------------------------------------------------------------------

/// What kind of line one [`DiffLine`] is, mirroring `git diff`'s ` `/`+`/`-`
/// origin markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

/// One line of a [`DiffHunk`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
}

/// One `@@ ... @@` hunk of a [`FileDiff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// All hunks for one file's uncommitted changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub status: FileStatus,
    pub hunks: Vec<DiffHunk>,
    pub binary: bool,
    /// `true` when [`MAX_DIFF_LINES_PER_FILE`] cut this file's hunks short
    /// — the UI should say so rather than silently rendering a partial diff
    /// as if it were the whole thing.
    pub truncated: bool,
}

/// Cap on diff lines collected per file (summed across all its hunks). A
/// diff of a huge generated/vendored file could otherwise be enormous; past
/// this, [`FileDiff::truncated`] is set instead of either dropping content
/// silently or materializing an unbounded string for the UI to render.
const MAX_DIFF_LINES_PER_FILE: usize = 2000;

/// Uncommitted changes in `worktree`: working tree + index vs. HEAD (an
/// unborn HEAD — no commits yet — diffs against an empty tree, so
/// everything shows as added, matching `git diff`'s own behavior there).
pub fn worktree_diff(worktree: &Path) -> Result<Vec<FileDiff>, String> {
    let repo = git2::Repository::open(worktree).map_err(|e| e.to_string())?;
    build_diffs(&repo, None)
}

/// The same as [`worktree_diff`], restricted to one file. `None` when
/// `rel_path` has no uncommitted changes.
pub fn file_diff(worktree: &Path, rel_path: &Path) -> Result<Option<FileDiff>, String> {
    if escapes_worktree(rel_path) {
        return Err(format!("'{}' escapes the worktree", rel_path.display()));
    }
    let repo = git2::Repository::open(worktree).map_err(|e| e.to_string())?;
    let pathspec = rel_path.to_string_lossy().into_owned();
    let diffs = build_diffs(&repo, Some(&pathspec))?;
    Ok(diffs.into_iter().next())
}

/// Build [`FileDiff`]s for `repo`'s working tree + index vs. HEAD, via
/// git2's diff API (`diff_tree_to_workdir_with_index`) rather than shelling
/// out to `git diff` and parsing text — this gets structured hunks and line
/// origins directly, which is both faster and less fragile. `only_path`,
/// when set, restricts the diff to an exact path (used by [`file_diff`]).
fn build_diffs(repo: &git2::Repository, only_path: Option<&str>) -> Result<Vec<FileDiff>, String> {
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());

    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        // Without this, untracked files get only their path recorded, not
        // their content -- no patch, no hunks, and binary detection can't
        // run. We want them to show up as all-added diffs, so the content
        // has to actually be read.
        .show_untracked_content(true)
        .context_lines(3);
    if let Some(p) = only_path {
        opts.pathspec(p).disable_pathspec_match(true);
    }

    let mut diff = repo
        .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))
        .map_err(|e| e.to_string())?;

    // Detect renames so a moved file shows as one Renamed entry instead of
    // a delete+add pair.
    let mut find_opts = git2::DiffFindOptions::new();
    find_opts.renames(true);
    diff.find_similar(Some(&mut find_opts))
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for idx in 0..diff.deltas().len() {
        let Some(delta) = diff.get_delta(idx) else {
            continue;
        };
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let status = delta_status(delta.status());

        // libgit2 only determines binary-ness once it actually reads the
        // file content, which happens lazily *inside* `git_patch_from_diff`
        // -- `delta.flags()` reads as plain text beforehand even for a
        // binary file. Build the patch first, then re-fetch the delta: the
        // diff's internal copy is updated in place by the patch call, so a
        // fresh `get_delta` (not the snapshot from before) is what actually
        // carries the `BINARY` flag. Never dump binary bytes into a UI
        // string.
        let patch = git2::Patch::from_diff(&diff, idx).map_err(|e| e.to_string())?;
        let is_binary = diff
            .get_delta(idx)
            .is_some_and(|d| d.flags().contains(git2::DiffFlags::BINARY));
        if is_binary {
            out.push(FileDiff {
                path,
                status,
                hunks: Vec::new(),
                binary: true,
                truncated: false,
            });
            continue;
        }
        let Some(patch) = patch else {
            // Not binary and no patch: nothing textual changed (e.g. a bare
            // mode change) -- an empty hunk list is the honest answer.
            out.push(FileDiff {
                path,
                status,
                hunks: Vec::new(),
                binary: false,
                truncated: false,
            });
            continue;
        };

        let mut hunks = Vec::new();
        let mut total_lines = 0usize;
        let mut truncated = false;
        for hunk_idx in 0..patch.num_hunks() {
            if truncated {
                break;
            }
            let (raw_hunk, _) = patch.hunk(hunk_idx).map_err(|e| e.to_string())?;
            let header = String::from_utf8_lossy(raw_hunk.header())
                .trim_end()
                .to_string();
            let line_count = patch
                .num_lines_in_hunk(hunk_idx)
                .map_err(|e| e.to_string())?;
            let mut lines = Vec::new();
            for line_idx in 0..line_count {
                if total_lines >= MAX_DIFF_LINES_PER_FILE {
                    truncated = true;
                    break;
                }
                let raw_line = patch
                    .line_in_hunk(hunk_idx, line_idx)
                    .map_err(|e| e.to_string())?;
                let kind = match raw_line.origin_value() {
                    git2::DiffLineType::Addition | git2::DiffLineType::AddEOFNL => {
                        DiffLineKind::Added
                    }
                    git2::DiffLineType::Deletion | git2::DiffLineType::DeleteEOFNL => {
                        DiffLineKind::Removed
                    }
                    _ => DiffLineKind::Context,
                };
                let text = String::from_utf8_lossy(raw_line.content())
                    .trim_end_matches('\n')
                    .to_string();
                lines.push(DiffLine {
                    kind,
                    text,
                    old_lineno: raw_line.old_lineno(),
                    new_lineno: raw_line.new_lineno(),
                });
                total_lines += 1;
            }
            if !lines.is_empty() {
                hunks.push(DiffHunk { header, lines });
            }
        }

        out.push(FileDiff {
            path,
            status,
            hunks,
            binary: false,
            truncated,
        });
    }

    Ok(out)
}

/// Map git2's delta status to our [`FileStatus`]. `Copied` folds into
/// `Added` (no dedicated variant); anything else unexpected in a diff of
/// changes (`Unmodified`/`Ignored`/`Unreadable`) degrades to `Modified`
/// rather than dropping the file from the result.
fn delta_status(status: git2::Delta) -> FileStatus {
    match status {
        git2::Delta::Added | git2::Delta::Copied => FileStatus::Added,
        git2::Delta::Untracked => FileStatus::Untracked,
        git2::Delta::Deleted => FileStatus::Deleted,
        git2::Delta::Renamed => FileStatus::Renamed,
        git2::Delta::Conflicted => FileStatus::Conflicted,
        _ => FileStatus::Modified,
    }
}

// ---------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------

/// Result of a [`fetch`] run: which remote it hit and how many refs it moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchOutcome {
    pub remote: String,
    pub updated_refs: usize,
}

/// Run `git fetch --prune` against `remote` (or the default remote when
/// `None`), updating this repo's remote-tracking refs.
///
/// Ahead/behind counts are computed against those refs, and prune's
/// "upstream gone" detection depends on them too — both are only ever as
/// honest as the last fetch, so this is what refreshes them.
///
/// Shells out to the `git` binary rather than using git2's own fetch. git2
/// would need credential callbacks re-implemented by hand: SSH agent
/// forwarding, macOS Keychain, `credential.helper` config. Get any of that
/// wrong (easy to do) and fetch breaks for anyone whose remote isn't a plain
/// unauthenticated HTTPS URL — in practice most SSH-keyed GitHub/GitLab
/// users. The system `git` binary already has all of that working
/// correctly; shelling out reuses it instead of reimplementing it worse.
///
/// `--prune` so branches deleted on the remote actually disappear from
/// remote-tracking refs here too — that's what makes `wtm prune`'s "upstream
/// gone" detection trustworthy instead of stale.
pub fn fetch(repo: &OpenRepo, remote: Option<&str>) -> Result<FetchOutcome, String> {
    let remote_name = match remote {
        Some(r) => r.to_string(),
        None => default_remote_name(&repo.ctx)?,
    };

    let output = gitcmd::run_capture(&repo.ctx.main_root, &["fetch", "--prune", &remote_name])
        .map_err(|e| e.to_string())?;

    // git's own progress/ref-update reporting all goes to stderr; stdout is
    // normally empty. Combine both so nothing is silently dropped regardless
    // of which stream a particular git version or transport happens to use.
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));

    if !output.status.success() {
        let trimmed = combined.trim();
        return Err(if trimmed.is_empty() {
            format!("git fetch exited with {}", output.status)
        } else {
            trimmed.to_string()
        });
    }

    Ok(FetchOutcome {
        updated_refs: count_updated_refs(&combined),
        remote: remote_name,
    })
}

/// The remote `fetch` uses when the caller doesn't name one: `origin` if
/// configured, else whichever remote sorts first alphabetically (a
/// deterministic choice among equals); an error naming the problem when
/// there are none.
fn default_remote_name(ctx: &RepoContext) -> Result<String, String> {
    let git_repo = ctx.open_main().map_err(|e| e.to_string())?;
    let mut names: Vec<String> = git_repo
        .remotes()
        .map_err(|e| e.to_string())?
        .iter()
        // Each entry is `Result<Option<&str>, Error>`: `Err` for a git-level
        // read failure, `Ok(None)` for a non-UTF-8 name. Neither is
        // something a remote picker can act on, so both are dropped rather
        // than failing the whole listing over one oddly named remote.
        .filter_map(|entry| entry.ok().flatten())
        .map(str::to_owned)
        .collect();
    if names.iter().any(|n| n == "origin") {
        return Ok("origin".to_string());
    }
    names.sort();
    names
        .into_iter()
        .next()
        .ok_or_else(|| "this repository has no configured remotes".to_string())
}

/// Count how many refs `git fetch`'s output reports as touched. Every line
/// git prints for an updated, new, or deleted ref ends in ` -> <local-ref>`
/// (e.g. `   1234abc..5678def  main       -> origin/main`,
/// ` * [new branch]      feat       -> origin/feat`,
/// ` - [deleted]         (none)     -> origin/old`); progress lines and the
/// leading `From <url>` line never take that shape. Not bulletproof against
/// a ref name that happens to contain the literal substring ` -> `, but good
/// enough for a UI count — and reporting 0 when nothing matches is the
/// honest answer rather than a guess.
fn count_updated_refs(output: &str) -> usize {
    output.lines().filter(|line| line.contains(" -> ")).count()
}

// ---------------------------------------------------------------------
// Worktree activity
// ---------------------------------------------------------------------

/// Unix seconds of the HEAD commit for each given worktree path. A worktree
/// that can't be opened or has no resolvable HEAD (unborn, corrupted, or
/// simply gone from disk since the caller listed it) is left out of the map
/// entirely rather than erroring the whole batch — one bad worktree
/// shouldn't blank out staleness for the rest of the table.
///
/// Cheap enough to call for every row on every listing: per path this is one
/// `git2::Repository::open`, one HEAD lookup, and one commit-object read —
/// no history walk and no status computation (that's `worktree::list`'s
/// `with_status`, a much more expensive pass). Same order of cost as
/// `stat`-ing a file, repeated once per worktree rather than per commit.
pub fn worktree_activity(paths: &[PathBuf]) -> HashMap<PathBuf, i64> {
    let mut activity = HashMap::with_capacity(paths.len());
    for path in paths {
        let Ok(repo) = git2::Repository::open(path) else {
            continue;
        };
        let Ok(head) = repo.head() else {
            continue; // Unborn HEAD (no commits yet), or the worktree is gone.
        };
        let Ok(commit) = head.peel_to_commit() else {
            continue;
        };
        activity.insert(path.clone(), commit.time().seconds());
    }
    activity
}

/// Format `unix_secs` relative to `now`: "just now", "5m", "3h", "2d", "3w",
/// "5mo", "2y". Same thresholds and rounding as `detail_panel`'s (private)
/// `relative_time` — duplicated here rather than shared because that
/// formatter is private to its module and this crate has no shared
/// "formatting" module yet to hoist it into.
///
/// `now < unix_secs` (a future timestamp — clock skew, or a commit made on a
/// machine with a fast clock) is not special-cased: the elapsed time comes
/// out negative, which is less than every threshold below, so it falls into
/// the same "just now" bucket as a genuinely recent commit rather than
/// printing a negative duration or panicking.
pub fn relative_age(unix_secs: i64, now: i64) -> String {
    let delta = now.saturating_sub(unix_secs);
    if delta < 60 {
        return "just now".to_string();
    }

    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;

    if delta < HOUR {
        format!("{}m", delta / MINUTE)
    } else if delta < DAY {
        format!("{}h", delta / HOUR)
    } else if delta < WEEK {
        format!("{}d", delta / DAY)
    } else if delta < MONTH {
        format!("{}w", delta / WEEK)
    } else if delta < YEAR {
        format!("{}mo", delta / MONTH)
    } else {
        format!("{}y", delta / YEAR)
    }
}

// ---------------------------------------------------------------------
// Run a command in a worktree
// ---------------------------------------------------------------------

/// One step of a [`run_command_streaming`] run, reported as it happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandEvent {
    Started {
        command: String,
    },
    /// One line of combined stdout/stderr, in arrival order. Interleaving
    /// between the two streams is best-effort (they're read on separate
    /// threads); ordering within a single stream is exact.
    Output {
        line: String,
    },
    Finished {
        success: bool,
        code: Option<i32>,
    },
}

/// Run `command` via `sh -c` with cwd = `worktree`, reporting each line of
/// output as it arrives instead of buffering until exit — the TUI does the
/// same thing for its `RunCommand` effect, just without streaming (it has a
/// terminal to inherit stdio into; the app doesn't). The shape here mirrors
/// `wtm::setup::run_streaming`'s own command runner: stdout and stderr are
/// each read on their own thread into one channel, and `stdin` is null so a
/// command that tries to prompt for input hits a closed pipe instead of
/// hanging forever waiting on a terminal that isn't there.
///
/// A non-zero exit is reported as `Finished { success: false, code }`, NOT
/// as `Err` — `Err` is reserved for "the command could not be started at
/// all" (e.g. `sh` itself is missing). Folding "ran and failed" into `Err`
/// would make a perfectly ordinary outcome — a failing test suite, a lint
/// error, a `grep` that found nothing — look identical to the app itself
/// being broken, to both callers matching on `Result` and to any
/// error-toast UI built on top of this.
pub fn run_command_streaming(
    worktree: &Path,
    command: &str,
    sink: &mut dyn FnMut(CommandEvent),
) -> Result<(), String> {
    sink(CommandEvent::Started {
        command: command.to_string(),
    });

    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(worktree)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("`{command}` could not be started: {e}"))?;

    let stdout = child.stdout.take().expect("stdout was piped at spawn");
    let stderr = child.stderr.take().expect("stderr was piped at spawn");
    let (tx, rx) = mpsc::channel::<String>();
    let tx_stderr = tx.clone();
    let stdout_reader = thread::spawn(move || wtm::setup::forward_lines(stdout, tx));
    let stderr_reader = thread::spawn(move || wtm::setup::forward_lines(stderr, tx_stderr));

    for line in rx {
        sink(CommandEvent::Output { line });
    }
    // Both reader threads have already dropped their `Sender` (the channel
    // just closed on its own), so they are done or finishing — this join is
    // not a stall.
    let _ = stdout_reader.join();
    let _ = stderr_reader.join();

    let status = child
        .wait()
        .map_err(|e| format!("`{command}` could not be waited on: {e}"))?;
    sink(CommandEvent::Finished {
        success: status.success(),
        code: status.code(),
    });
    Ok(())
}

// ---------------------------------------------------------------------
// Open the branch on its remote host
// ---------------------------------------------------------------------

/// A browsable URL for `branch` on the worktree's remote, if one can be
/// derived: `branch`'s own upstream remote when it has one, else `origin`,
/// converted from its SSH or HTTPS form into an `https://` base and pointed
/// at the branch. `None` when the repo has no remote at all, or the remote
/// URL isn't in a recognized SSH/HTTPS shape.
///
/// Thin git-facing wrapper: [`resolve_remote_url`] reads the raw remote URL
/// via git2, [`build_remote_branch_url`] (pure, unit tested directly) does
/// the actual conversion.
pub fn remote_branch_url(repo: &OpenRepo, branch: &str) -> Option<String> {
    let git_repo = repo.ctx.open_main().ok()?;
    let raw_url = resolve_remote_url(&git_repo, branch);
    build_remote_branch_url(raw_url.as_deref(), branch)
}

/// The URL of `branch`'s own upstream remote (`branch.<branch>.remote`) if
/// it has one and that remote still exists in `git_repo`; otherwise
/// `origin`'s URL; otherwise `None` (no remote at all, or the resolved
/// remote's URL isn't set).
fn resolve_remote_url(git_repo: &git2::Repository, branch: &str) -> Option<String> {
    let upstream_remote = git_repo
        .branch_upstream_remote(&format!("refs/heads/{branch}"))
        .ok()
        .and_then(|buf| buf.as_str().ok().map(str::to_owned));

    let remote = upstream_remote
        .and_then(|name| git_repo.find_remote(&name).ok())
        .or_else(|| git_repo.find_remote("origin").ok())?;
    remote.url().ok().map(str::to_owned)
}

/// Pure core of [`remote_branch_url`]: given the raw remote URL (`None` when
/// there is no remote) and the branch name, produce a browsable URL.
/// Unit tested directly over a table of inputs since it needs no repository
/// at all.
fn build_remote_branch_url(raw_url: Option<&str>, branch: &str) -> Option<String> {
    let base = remote_url_to_https_base(raw_url?)?;
    Some(branch_url_for_host(&base, branch))
}

/// Convert an SSH or HTTPS git remote URL into an `https://host/owner/repo`
/// base (no trailing slash, no `.git` suffix). Handles the scp-like SSH form
/// (`git@github.com:owner/repo.git`), the explicit `ssh://` form
/// (`ssh://git@host/owner/repo.git`), and `https://`/`http://` forms.
/// `None` for anything else (e.g. a local filesystem path) — there is no
/// sane host to build a browsable URL from.
fn remote_url_to_https_base(url: &str) -> Option<String> {
    let url = url.trim();

    if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        let (host, path) = rest.split_once('/')?;
        return build_https_base(host, path);
    }
    if let Some(rest) = url.strip_prefix("ssh://") {
        let rest = rest
            .split_once('@')
            .map_or(rest, |(_, host_and_path)| host_and_path);
        let (host_and_port, path) = rest.split_once('/')?;
        let host = host_and_port
            .split_once(':')
            .map_or(host_and_port, |(h, _)| h);
        return build_https_base(host, path);
    }
    // scp-like syntax: `[user@]host:path`. Only treated as such when there's
    // no `/` before the `:` — otherwise an already-handled `scheme://` URL,
    // or some unrelated string with a colon in it, could be misread as this
    // form.
    let (user_host, path) = url.split_once(':')?;
    if user_host.contains('/') || path.is_empty() {
        return None;
    }
    let host = user_host.split_once('@').map_or(user_host, |(_, h)| h);
    build_https_base(host, path)
}

/// Join a host and a `owner/repo[.git]` path into an `https://` base,
/// stripping a trailing `.git` and any stray leading/trailing slashes.
/// `None` if either half is empty once trimmed.
fn build_https_base(host: &str, path: &str) -> Option<String> {
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("https://{host}/{path}"))
}

/// Append the host-appropriate branch path onto `base` (an
/// `https://host/owner/repo` URL from [`remote_url_to_https_base`]).
/// GitHub and GitLab both browse a branch at `/tree/<branch>`; Bitbucket at
/// `/src/<branch>`. Matching is by exact host, not a suffix/substring check,
/// so a self-hosted look-alike (e.g. `git.mycompany.com`) doesn't get
/// guessed into a GitLab-shaped link that might not exist there — it falls
/// back to `base` unchanged, which at least works.
fn branch_url_for_host(base: &str, branch: &str) -> String {
    let host = base
        .strip_prefix("https://")
        .and_then(|rest| rest.split('/').next());
    let encoded_branch = encode_branch_path(branch);
    match host {
        Some("github.com") | Some("gitlab.com") => format!("{base}/tree/{encoded_branch}"),
        Some("bitbucket.org") => format!("{base}/src/{encoded_branch}"),
        _ => base.to_string(),
    }
}

/// Percent-encode `branch` for use as a URL path, one `/`-separated segment
/// at a time: the `/` itself is preserved as a path separator (a branch
/// like `feature/x` should still browse as two path segments, not one
/// encoded blob — GitHub/GitLab both resolve it that way), while everything
/// else outside the URL-safe unreserved set (letters, digits, `-`, `.`, `_`,
/// `~`) is percent-encoded — notably `+`, which some URL consumers would
/// otherwise decode as a space.
fn encode_branch_path(branch: &str) -> String {
    branch
        .split('/')
        .map(encode_path_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn encode_path_segment(segment: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => write!(out, "%{byte:02X}").unwrap(),
        }
    }
    out
}

/// Open `url` in the system's default browser: `open <url>` on macOS,
/// `xdg-open <url>` elsewhere. Does not decide *when* to open anything —
/// launching a browser is a UI decision made on user action, not something
/// the data layer initiates on its own.
pub fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let tool = "open";
    #[cfg(not(target_os = "macos"))]
    let tool = "xdg-open";

    std::process::Command::new(tool)
        .arg(url)
        .status()
        .map_err(|e| format!("could not launch {tool}: {e}"))
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(format!("{tool} exited with {status}"))
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    // ---- shared git fixture helper (same pattern as worktree.rs's tests) ----

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "wtm test")
            .env("GIT_AUTHOR_EMAIL", "wtm@example.invalid")
            .env("GIT_COMMITTER_NAME", "wtm test")
            .env("GIT_COMMITTER_EMAIL", "wtm@example.invalid")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("failed to run git");
        assert!(
            out.status.success(),
            "git {:?} failed:\n{}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(dir: &Path) {
        git(dir, &["init", "-b", "main"]);
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-m", "initial"]);
    }

    /// `main/` inside a fresh tmp dir, initialized as a repo with one commit.
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let main = tmp.path().join("main");
        std::fs::create_dir(&main).unwrap();
        init_repo(&main);
        (tmp, main)
    }

    /// An [`OpenRepo`] for `main`, with a default (not the host machine's
    /// real global) config — determinism matters more here than exercising
    /// `config::load`, which has its own tests.
    fn test_repo(main: &Path) -> OpenRepo {
        let ctx = repo::discover(Some(main)).unwrap();
        OpenRepo {
            ctx,
            config: Config::default(),
        }
    }

    // ---------------- Task 1: reveal_in_finder ----------------

    #[test]
    fn existing_ancestor_of_a_dir_is_itself() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(
            existing_ancestor(tmp.path()),
            Some(tmp.path().to_path_buf())
        );
    }

    #[test]
    fn existing_ancestor_of_a_file_is_itself() {
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(existing_ancestor(&file), Some(file));
    }

    #[test]
    fn existing_ancestor_of_missing_file_is_its_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let missing = tmp.path().join("does-not-exist.toml");
        assert_eq!(existing_ancestor(&missing), Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn existing_ancestor_none_when_all_ancestors_missing() {
        let path = Path::new("definitely/does/not/exist/anywhere/at/all");
        assert_eq!(existing_ancestor(path), None);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn xdg_open_fallback_target_is_the_dir_itself_for_a_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(xdg_open_fallback_target(tmp.path()), tmp.path());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn xdg_open_fallback_target_is_the_parent_for_a_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(xdg_open_fallback_target(&file), tmp.path());
    }

    // ---------------- Task: open_in_terminal (Linux) ----------------

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn terminal_args_uses_equals_form_for_gnome_terminal_and_foot() {
        let path = Path::new("/repo/wt");
        assert_eq!(
            terminal_args("gnome-terminal", path),
            vec![std::ffi::OsString::from("--working-directory=/repo/wt")]
        );
        assert_eq!(
            terminal_args("foot", path),
            vec![std::ffi::OsString::from("--working-directory=/repo/wt")]
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn terminal_args_uses_separate_flag_and_value_for_konsole_alacritty_kitty() {
        let path = Path::new("/repo/wt");
        assert_eq!(
            terminal_args("konsole", path),
            vec![
                std::ffi::OsString::from("--workdir"),
                std::ffi::OsString::from("/repo/wt")
            ]
        );
        assert_eq!(
            terminal_args("alacritty", path),
            vec![
                std::ffi::OsString::from("--working-directory"),
                std::ffi::OsString::from("/repo/wt")
            ]
        );
        assert_eq!(
            terminal_args("kitty", path),
            vec![
                std::ffi::OsString::from("--directory"),
                std::ffi::OsString::from("/repo/wt")
            ]
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn terminal_args_wezterm_uses_start_cwd_subcommand() {
        let path = Path::new("/repo/wt");
        assert_eq!(
            terminal_args("wezterm", path),
            vec![
                std::ffi::OsString::from("start"),
                std::ffi::OsString::from("--cwd"),
                std::ffi::OsString::from("/repo/wt")
            ]
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn terminal_args_xterm_and_x_terminal_emulator_rely_on_inherited_cwd() {
        let path = Path::new("/repo/wt");
        assert!(terminal_args("xterm", path).is_empty());
        assert!(terminal_args("x-terminal-emulator", path).is_empty());
        // An unrecognized name (e.g. a custom $WTM_TERMINAL) degrades the
        // same way rather than guessing at flags it might not support.
        assert!(terminal_args("some-custom-term", path).is_empty());
    }

    // ---------------- Task 2: list_refs ordering ----------------

    fn raw_local(name: &str) -> RawRef {
        RawRef {
            name: name.to_string(),
            remote: None,
        }
    }

    fn raw_remote(remote: &str, short: &str) -> RawRef {
        RawRef {
            name: format!("{remote}/{short}"),
            remote: Some(remote.to_string()),
        }
    }

    #[test]
    fn orders_current_default_locals_then_remotes() {
        let refs = vec![
            raw_local("zeta"),
            raw_local("alpha"),
            raw_local("beta"),
            raw_remote("origin", "main"),
            raw_remote("origin", "zeta"),
        ];
        let elsewhere = HashSet::new();
        let out = assemble_ref_order(refs, Some("beta"), &elsewhere, None);

        let names: Vec<&str> = out.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "beta",
                "HEAD",
                "alpha",
                "zeta",
                "origin/main",
                "origin/zeta"
            ]
        );
        assert_eq!(out[0].kind, RefKind::Current);
        assert_eq!(out[1].kind, RefKind::Default);
        assert_eq!(out[2].kind, RefKind::Local);
        assert_eq!(out[3].kind, RefKind::Local);
        assert_eq!(
            out[4].kind,
            RefKind::Remote {
                remote: "origin".to_string()
            }
        );
    }

    #[test]
    fn default_promotes_existing_local_branch_instead_of_duplicating() {
        let refs = vec![raw_local("main"), raw_local("feature")];
        let elsewhere = HashSet::new();
        let out = assemble_ref_order(refs, None, &elsewhere, Some("main"));

        assert_eq!(out.len(), 2, "no separate synthetic HEAD entry: {out:?}");
        assert_eq!(out[0].name, "main");
        assert_eq!(out[0].kind, RefKind::Default);
        assert_eq!(out[1].name, "feature");
        assert_eq!(out[1].kind, RefKind::Local);
    }

    #[test]
    fn default_promotes_existing_remote_branch_instead_of_duplicating() {
        let refs = vec![raw_local("feature"), raw_remote("origin", "main")];
        let elsewhere = HashSet::new();
        let out = assemble_ref_order(refs, None, &elsewhere, Some("origin/main"));

        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "origin/main");
        assert_eq!(out[0].kind, RefKind::Default);
        assert_eq!(out[1].name, "feature");
        assert_eq!(out[1].kind, RefKind::Local);
    }

    #[test]
    fn current_equal_to_default_yields_one_entry_not_two() {
        let refs = vec![raw_local("main")];
        let elsewhere = HashSet::new();
        let out = assemble_ref_order(refs, Some("main"), &elsewhere, Some("main"));

        assert_eq!(out.len(), 1, "expected one row, not a duplicate: {out:?}");
        assert_eq!(out[0].kind, RefKind::Current);
    }

    #[test]
    fn worktree_kind_for_branch_checked_out_elsewhere() {
        let refs = vec![raw_local("main"), raw_local("feature")];
        let mut elsewhere = HashSet::new();
        elsewhere.insert("feature".to_string());
        let out = assemble_ref_order(refs, Some("main"), &elsewhere, None);

        assert_eq!(out[0].name, "main");
        assert_eq!(out[0].kind, RefKind::Current);
        assert_eq!(out[1].name, "HEAD");
        assert_eq!(out[1].kind, RefKind::Default);
        assert_eq!(out[2].name, "feature");
        assert_eq!(out[2].kind, RefKind::Worktree);
    }

    #[test]
    fn list_refs_end_to_end_against_a_real_repo() {
        let (_tmp, main) = fixture();
        git(&main, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&main, &["branch", "feature"]);

        let repo = test_repo(&main);
        let refs = list_refs(&repo, Some(&main)).unwrap();

        let current = refs.iter().find(|r| r.name == "main").unwrap();
        assert_eq!(current.kind, RefKind::Current);
        assert!(current.subject.is_some());

        let feature = refs.iter().find(|r| r.name == "feature").unwrap();
        assert_eq!(feature.kind, RefKind::Local);

        let remote = refs.iter().find(|r| r.name == "origin/main").unwrap();
        assert_eq!(
            remote.kind,
            RefKind::Remote {
                remote: "origin".to_string()
            }
        );

        // default_base is unset -> synthesized "HEAD" entry, distinct from
        // "main" since HEAD is resolved as its own ref, not folded into the
        // branch it happens to point at.
        let default = refs.iter().find(|r| r.kind == RefKind::Default).unwrap();
        assert_eq!(default.name, "HEAD");
        assert!(default.subject.is_some());
    }

    // ---------------- Task 3: list_files ----------------

    #[test]
    fn escapes_worktree_rejects_any_parent_dir_component() {
        assert!(escapes_worktree(Path::new("../etc")));
        assert!(escapes_worktree(Path::new("a/../../b")));
        assert!(!escapes_worktree(Path::new("a/b")));
        assert!(!escapes_worktree(Path::new("")));
    }

    fn file_entry(name: &str, is_dir: bool) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            rel_path: PathBuf::from(name),
            is_dir,
            status: None,
        }
    }

    #[test]
    fn sort_file_entries_puts_dirs_first_case_insensitively() {
        let mut entries = vec![
            file_entry("zeta.txt", false),
            file_entry("Banana", true),
            file_entry("apple.txt", false),
            file_entry("alpha", true),
        ];
        sort_file_entries(&mut entries);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "Banana", "apple.txt", "zeta.txt"]);
    }

    #[test]
    fn list_files_rejects_escaping_rel_dir() {
        let (_tmp, main) = fixture();
        let err = list_files(&main, Path::new("../escape")).unwrap_err();
        assert!(err.contains("escapes"), "{err}");
    }

    #[test]
    fn list_files_respects_gitignore_and_attaches_status() {
        let (_tmp, main) = fixture();

        std::fs::write(main.join(".gitignore"), "target/\nignored.txt\n").unwrap();
        git(&main, &["add", ".gitignore"]);
        git(&main, &["commit", "-m", "gitignore"]);

        std::fs::create_dir(main.join("target")).unwrap();
        std::fs::write(main.join("target").join("build.o"), "x").unwrap();
        std::fs::write(main.join("ignored.txt"), "x").unwrap();
        std::fs::write(main.join("new.txt"), "x").unwrap();
        std::fs::create_dir(main.join("src")).unwrap();
        std::fs::write(main.join("src").join("lib.rs"), "fn a() {}\n").unwrap();
        std::fs::write(main.join("README.md"), "changed\n").unwrap();

        let entries = list_files(&main, Path::new("")).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"target"),
            "gitignored dir must not be listed: {names:?}"
        );
        assert!(
            !names.contains(&"ignored.txt"),
            "gitignored file must not be listed: {names:?}"
        );
        assert!(names.contains(&"src"));
        assert!(names.contains(&"new.txt"));

        let readme = entries.iter().find(|e| e.name == "README.md").unwrap();
        assert_eq!(readme.status, Some(FileStatus::Modified));
        let new_file = entries.iter().find(|e| e.name == "new.txt").unwrap();
        assert_eq!(new_file.status, Some(FileStatus::Untracked));

        // Directories sort before files.
        let src_pos = entries.iter().position(|e| e.name == "src").unwrap();
        let new_pos = entries.iter().position(|e| e.name == "new.txt").unwrap();
        assert!(src_pos < new_pos);
    }

    // ---------------- Task 4: worktree_diff / file_diff ----------------

    #[test]
    fn worktree_diff_reports_hunks_with_line_numbers() {
        let (_tmp, main) = fixture();
        std::fs::write(main.join("README.md"), "hello\nworld\n").unwrap();

        let diffs = worktree_diff(&main).unwrap();
        let readme = diffs.iter().find(|d| d.path == "README.md").unwrap();
        assert_eq!(readme.status, FileStatus::Modified);
        assert!(!readme.binary);
        assert_eq!(readme.hunks.len(), 1);

        let added: Vec<&DiffLine> = readme.hunks[0]
            .lines
            .iter()
            .filter(|l| l.kind == DiffLineKind::Added)
            .collect();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].text, "world");
        assert_eq!(added[0].new_lineno, Some(2));
        assert_eq!(added[0].old_lineno, None);
    }

    #[test]
    fn worktree_diff_shows_untracked_file_as_all_added() {
        let (_tmp, main) = fixture();
        std::fs::write(main.join("new.txt"), "one\ntwo\n").unwrap();

        let diffs = worktree_diff(&main).unwrap();
        let new_file = diffs.iter().find(|d| d.path == "new.txt").unwrap();
        assert_eq!(new_file.status, FileStatus::Untracked);
        assert!(!new_file.hunks.is_empty());
        assert!(new_file
            .hunks
            .iter()
            .all(|h| h.lines.iter().all(|l| l.kind == DiffLineKind::Added)));
    }

    #[test]
    fn worktree_diff_marks_binary_files_without_hunks() {
        let (_tmp, main) = fixture();
        std::fs::write(main.join("bin.dat"), [0u8, 159, 146, 150, 0, 1, 2, 3]).unwrap();

        let diffs = worktree_diff(&main).unwrap();
        let bin = diffs.iter().find(|d| d.path == "bin.dat").unwrap();
        assert!(bin.binary);
        assert!(bin.hunks.is_empty());
    }

    #[test]
    fn file_diff_restricts_to_one_file() {
        let (_tmp, main) = fixture();
        std::fs::write(main.join("README.md"), "hello\nworld\n").unwrap();
        std::fs::write(main.join("other.txt"), "x").unwrap();

        let diff = file_diff(&main, Path::new("README.md")).unwrap().unwrap();
        assert_eq!(diff.path, "README.md");

        assert!(file_diff(&main, Path::new("does-not-exist.txt"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn file_diff_rejects_escaping_rel_path() {
        let (_tmp, main) = fixture();
        let err = file_diff(&main, Path::new("../escape.txt")).unwrap_err();
        assert!(err.contains("escapes"), "{err}");
    }

    #[test]
    fn worktree_diff_truncates_huge_files() {
        let (_tmp, main) = fixture();
        let big: String = (0..3000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(main.join("big.txt"), big).unwrap();

        let diffs = worktree_diff(&main).unwrap();
        let big_file = diffs.iter().find(|d| d.path == "big.txt").unwrap();
        assert!(big_file.truncated);
        let total_lines: usize = big_file.hunks.iter().map(|h| h.lines.len()).sum();
        assert_eq!(total_lines, MAX_DIFF_LINES_PER_FILE);
    }

    // ---------------- Task 5: fetch ----------------

    #[test]
    fn count_updated_refs_counts_arrow_lines_only() {
        let output = "\
From github.com:owner/repo
   1234abc..5678def  main       -> origin/main
 * [new branch]      feat       -> origin/feat
 - [deleted]         (none)     -> origin/old
Fetching origin
";
        assert_eq!(count_updated_refs(output), 3);
    }

    #[test]
    fn count_updated_refs_is_honestly_zero_when_nothing_changed() {
        assert_eq!(count_updated_refs("From github.com:owner/repo\n"), 0);
        assert_eq!(count_updated_refs(""), 0);
    }

    #[test]
    fn default_remote_name_prefers_origin() {
        let (_tmp, main) = fixture();
        git(
            &main,
            &[
                "remote",
                "add",
                "zzz-other",
                "https://example.invalid/z.git",
            ],
        );
        git(
            &main,
            &["remote", "add", "origin", "https://example.invalid/o.git"],
        );

        let ctx = repo::discover(Some(&main)).unwrap();
        assert_eq!(default_remote_name(&ctx).unwrap(), "origin");
    }

    #[test]
    fn default_remote_name_falls_back_to_first_alphabetically() {
        let (_tmp, main) = fixture();
        git(
            &main,
            &["remote", "add", "zzz", "https://example.invalid/z.git"],
        );
        git(
            &main,
            &["remote", "add", "aaa", "https://example.invalid/a.git"],
        );

        let ctx = repo::discover(Some(&main)).unwrap();
        assert_eq!(default_remote_name(&ctx).unwrap(), "aaa");
    }

    #[test]
    fn default_remote_name_errors_clearly_with_no_remotes() {
        let (_tmp, main) = fixture();
        let ctx = repo::discover(Some(&main)).unwrap();
        let err = default_remote_name(&ctx).unwrap_err();
        assert!(err.contains("no configured remotes"), "{err}");
    }

    // ---------------- Task 6: worktree_activity / relative_age ----------------

    #[test]
    fn worktree_activity_reports_head_commit_time_and_skips_missing_paths() {
        let (_tmp, main) = fixture();
        let missing = main.parent().unwrap().join("does-not-exist");

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let activity = worktree_activity(&[main.clone(), missing.clone()]);
        assert!(!activity.contains_key(&missing));
        let commit_time = *activity.get(&main).expect("main worktree must be present");
        // The fixture's commit was just made; allow generous slack for slow
        // CI clocks rather than asserting an exact timestamp.
        assert!(
            (now - commit_time).abs() < 300,
            "commit_time={commit_time} now={now}"
        );
    }

    const NOW: i64 = 1_700_000_000;

    #[test]
    fn relative_age_zero_delta_is_just_now() {
        assert_eq!(relative_age(NOW, NOW), "just now");
    }

    #[test]
    fn relative_age_future_timestamp_clamps_to_just_now() {
        // Clock skew: the commit's timestamp is ahead of `now`. Must not
        // panic or print a negative duration.
        assert_eq!(relative_age(NOW + 3600, NOW), "just now");
        assert_eq!(relative_age(i64::MAX, NOW), "just now");
    }

    #[test]
    fn relative_age_seconds_are_just_now() {
        assert_eq!(relative_age(NOW - 1, NOW), "just now");
        assert_eq!(relative_age(NOW - 59, NOW), "just now");
    }

    #[test]
    fn relative_age_minute_boundary() {
        assert_eq!(relative_age(NOW - 60, NOW), "1m");
        assert_eq!(relative_age(NOW - 3599, NOW), "59m");
    }

    #[test]
    fn relative_age_hour_boundary() {
        assert_eq!(relative_age(NOW - 3600, NOW), "1h");
        assert_eq!(relative_age(NOW - 86399, NOW), "23h");
    }

    #[test]
    fn relative_age_day_boundary() {
        assert_eq!(relative_age(NOW - 86400, NOW), "1d");
        assert_eq!(relative_age(NOW - (7 * 86400 - 1), NOW), "6d");
    }

    #[test]
    fn relative_age_week_boundary() {
        assert_eq!(relative_age(NOW - 7 * 86400, NOW), "1w");
        assert_eq!(relative_age(NOW - (30 * 86400 - 1), NOW), "4w");
    }

    #[test]
    fn relative_age_month_boundary() {
        assert_eq!(relative_age(NOW - 30 * 86400, NOW), "1mo");
        assert_eq!(relative_age(NOW - (365 * 86400 - 1), NOW), "12mo");
    }

    #[test]
    fn relative_age_year_boundary() {
        assert_eq!(relative_age(NOW - 365 * 86400, NOW), "1y");
        assert_eq!(relative_age(NOW - 2 * 365 * 86400, NOW), "2y");
    }

    // ---------------- Task 7: run_command_streaming ----------------

    #[test]
    fn run_command_streaming_reports_output_and_success() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut events = Vec::new();
        run_command_streaming(tmp.path(), "printf 'hello\\n'", &mut |e| events.push(e)).unwrap();

        assert_eq!(
            events,
            vec![
                CommandEvent::Started {
                    command: "printf 'hello\\n'".to_string()
                },
                CommandEvent::Output {
                    line: "hello".to_string()
                },
                CommandEvent::Finished {
                    success: true,
                    code: Some(0),
                },
            ]
        );
    }

    #[test]
    fn run_command_streaming_reports_nonzero_exit_as_finished_not_err() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut events = Vec::new();
        run_command_streaming(tmp.path(), "echo hello && exit 3", &mut |e| events.push(e))
            .expect("a failing command is not an Err — only a failed spawn is");

        assert_eq!(
            events,
            vec![
                CommandEvent::Started {
                    command: "echo hello && exit 3".to_string()
                },
                CommandEvent::Output {
                    line: "hello".to_string()
                },
                CommandEvent::Finished {
                    success: false,
                    code: Some(3),
                },
            ]
        );
    }

    #[test]
    fn run_command_streaming_runs_in_the_given_cwd() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("marker.txt"), "x").unwrap();
        let mut events = Vec::new();
        run_command_streaming(tmp.path(), "ls marker.txt", &mut |e| events.push(e)).unwrap();

        assert!(events
            .iter()
            .any(|e| matches!(e, CommandEvent::Output { line } if line == "marker.txt")));
    }

    // ---------------- Task 8: remote_branch_url ----------------

    #[test]
    fn build_remote_branch_url_over_a_table_of_inputs() {
        let cases: &[(Option<&str>, &str, Option<&str>)] = &[
            // scp-like SSH form.
            (
                Some("git@github.com:owner/repo.git"),
                "main",
                Some("https://github.com/owner/repo/tree/main"),
            ),
            // explicit ssh:// form.
            (
                Some("ssh://git@github.com/owner/repo.git"),
                "main",
                Some("https://github.com/owner/repo/tree/main"),
            ),
            // https with .git suffix.
            (
                Some("https://github.com/owner/repo.git"),
                "main",
                Some("https://github.com/owner/repo/tree/main"),
            ),
            // https without .git suffix.
            (
                Some("https://github.com/owner/repo"),
                "main",
                Some("https://github.com/owner/repo/tree/main"),
            ),
            // gitlab.com uses /tree/ too.
            (
                Some("git@gitlab.com:owner/repo.git"),
                "main",
                Some("https://gitlab.com/owner/repo/tree/main"),
            ),
            // bitbucket uses /src/.
            (
                Some("git@bitbucket.org:owner/repo.git"),
                "main",
                Some("https://bitbucket.org/owner/repo/src/main"),
            ),
            // self-hosted / unrecognized host -> base URL, no guessed path.
            (
                Some("git@git.mycompany.internal:team/proj.git"),
                "main",
                Some("https://git.mycompany.internal/team/proj"),
            ),
            // branch containing a slash and a `+` -> percent-encoded per
            // segment, slash preserved as a path separator.
            (
                Some("https://github.com/owner/repo.git"),
                "feature/a+b",
                Some("https://github.com/owner/repo/tree/feature/a%2Bb"),
            ),
            // no remote at all.
            (None, "main", None),
            // not a recognized SSH/HTTPS shape.
            (Some("/local/path/to/repo"), "main", None),
        ];

        for (raw_url, branch, expected) in cases {
            let actual = build_remote_branch_url(*raw_url, branch);
            assert_eq!(
                actual.as_deref(),
                *expected,
                "raw_url={raw_url:?} branch={branch}"
            );
        }
    }

    #[test]
    fn remote_branch_url_against_a_real_repo() {
        let (_tmp, main) = fixture();
        git(
            &main,
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:codenameakshay/wtm-manager.git",
            ],
        );

        let repo = test_repo(&main);
        let url = remote_branch_url(&repo, "main").unwrap();
        assert_eq!(
            url,
            "https://github.com/codenameakshay/wtm-manager/tree/main"
        );
    }
}

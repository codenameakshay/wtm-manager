//! The bridge between the window and the `wtm` library.
//!
//! Every function here is blocking and calls straight into the shared cores
//! (`wtm::worktree`, `wtm::commands::*`). None of it may run on the main
//! thread: `git2` status computation walks the working tree, and creating a
//! worktree shells out to `git`. Views call these through
//! [`gpui::AppContext::background_spawn`] and apply the result back on the
//! foreground.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

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
    rows: Vec<WorktreeInfo>,
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

/// Reveal a worktree in Finder.
pub fn reveal_in_finder(path: &Path) -> Result<(), String> {
    std::process::Command::new("open")
        .arg("-R")
        .arg(path)
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

/// Open a worktree in a terminal app. macOS only for now: `$WTM_TERMINAL`
/// names the app (`open -a <app> <path>`), falling back to `Terminal`.
pub fn open_in_terminal(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let terminal = std::env::var("WTM_TERMINAL").unwrap_or_else(|_| "Terminal".to_string());
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
        let _ = path;
        Err("opening a terminal is not supported on this platform yet".to_string())
    }
}

/// Copy `text` to the system clipboard: `pbcopy` on macOS; elsewhere the
/// first available of `wl-copy`, `xclip -selection clipboard`, `xsel -ib`
/// (same fallback chain as `wtm::tui`'s clipboard action, minus its OSC 52
/// terminal escape-sequence fallback — there is no terminal here to write
/// one into).
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let tools: &[(&str, &[&str])] = &[("pbcopy", &[])];
    #[cfg(not(target_os = "macos"))]
    let tools: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["-ib"]),
    ];

    for (tool, args) in tools {
        let child = std::process::Command::new(tool)
            .args(*args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            continue; // Not installed; try the next tool.
        };
        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            if stdin.write_all(text.as_bytes()).is_err() {
                continue;
            }
        }
        drop(child.stdin.take());
        match child.wait() {
            Ok(status) if status.success() => return Ok(()),
            _ => continue,
        }
    }

    let tried: Vec<&str> = tools.iter().map(|(tool, _)| *tool).collect();
    Err(format!(
        "no clipboard tool available (tried: {})",
        tried.join(", ")
    ))
}

/// One branch as shown in the create-worktree dialog's branch picker.
#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub name: String,
    pub is_local: bool,
    /// Already checked out in some worktree of this repository (a `wtm add`
    /// for it would fail with `BranchInUse`).
    pub is_checked_out: bool,
    /// Local branches only: has upstream configuration but the upstream ref
    /// no longer exists (same semantics as `WorktreeStatus::upstream_gone`).
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
        let upstream_gone = match branch.upstream() {
            Ok(_) => false,
            Err(e) if e.code() == git2::ErrorCode::NotFound => git_repo
                .config()
                .and_then(|cfg| cfg.get_string(&format!("branch.{name}.merge")))
                .is_ok(),
            Err(_) => false,
        };
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
    // The two loops above already build `locals` and `remotes` separately —
    // each internally alphabetical — before this concatenates them, so
    // locals already precede remotes. Re-deriving that ordering here from
    // `is_local` (a stable sort, so it only ever reorders across the
    // local/remote boundary, never within either group) ties it directly to
    // the field instead of leaving it as an implicit consequence of loop
    // order, so a future refactor that merges the two loops cannot silently
    // interleave local and remote branches.
    locals.sort_by_key(|b| !b.is_local);
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

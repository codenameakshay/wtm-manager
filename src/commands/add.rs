//! `wtm add` — create a worktree for an existing or new branch.
//!
//! The creation flow itself lives in the `create` core so that every
//! frontend — the CLI command, the TUI create form, and the GUI — shares one
//! implementation (checks, destination resolution, git invocation, setup
//! automation).

use std::path::{Path, PathBuf};

use crate::cli::{AddArgs, GlobalArgs};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::repo::RepoContext;
use crate::setup::SetupEvent;
use crate::template::{self, TemplateContext};
use crate::worktree::{self, ListOptions};
use crate::{gitcmd, setup};

/// Everything [`create`] needs beyond repo/config. The CLI fills this from
/// `AddArgs`; the TUI and GUI fill it from their create forms (with
/// `announce`/`cd` off, since they own the terminal/window and never want
/// stdout output).
pub struct CreateRequest<'a> {
    /// Branch to check out (created from the base ref when it does not
    /// exist yet).
    pub branch: &'a str,
    /// Base ref override for a newly created branch (`--from` / TUI form);
    /// `None` falls back to the configured `default_base`, then HEAD.
    pub base_override: Option<&'a str>,
    /// Destination override (`--path`); `None` renders the path template.
    pub path_override: Option<&'a Path>,
    /// Write the cd file (`--cd`) so the shell wrapper changes directory.
    pub cd: bool,
    /// Run the configured post-create setup (copy entries and commands).
    pub run_setup: bool,
    /// Print the "Created worktree ..." success line on stdout.
    pub announce: bool,
    pub quiet: bool,
    pub verbose: bool,
}

/// Create a worktree for `args.branch`, creating the branch when needed.
pub fn run(args: &AddArgs, global: &GlobalArgs) -> Result<()> {
    let (ctx, config) = super::prepare(global)?;

    let request = CreateRequest {
        branch: &args.branch,
        base_override: args.from.as_deref(),
        path_override: args.path.as_deref(),
        cd: args.cd,
        run_setup: !args.no_setup,
        announce: !global.quiet,
        quiet: global.quiet,
        verbose: global.verbose,
    };
    let dest = create(&ctx, &config, &request)?;

    if args.open {
        super::open::spawn_editor(&config, &dest)?;
        if !global.quiet {
            eprintln!("opened {} in your editor", dest.display());
        }
    }

    Ok(())
}

/// Create a worktree for `req.branch`, running post-create setup with
/// inherited stdio (setup command output goes straight to the CLI/TUI's own
/// stdout/stderr). See [`create_streaming`] for the GUI variant that
/// captures setup output instead of inheriting it; both share
/// `create_core` for everything before setup runs.
pub fn create(ctx: &RepoContext, config: &Config, req: &CreateRequest) -> Result<PathBuf> {
    let dest = create_core(ctx, config, req)?;
    if req.run_setup {
        setup::run(config, &ctx.main_root, &dest, req.quiet)?;
    }
    finish_cd(&dest, req)?;
    Ok(dest)
}

/// Create a worktree for `req.branch` exactly like [`create`], but run
/// post-create setup through [`setup::run_streaming`] so its steps and
/// command output are reported through `sink` instead of inherited — for
/// callers with no stdio to inherit into (the GUI).
pub fn create_streaming(
    ctx: &RepoContext,
    config: &Config,
    req: &CreateRequest,
    sink: &mut dyn FnMut(SetupEvent),
) -> Result<PathBuf> {
    let dest = create_core(ctx, config, req)?;
    if req.run_setup {
        setup::run_streaming(config, &ctx.main_root, &dest, sink)?;
    }
    finish_cd(&dest, req)?;
    Ok(dest)
}

/// Shared creation core: branch-in-use pre-check, destination resolution,
/// branch-exists check, `git worktree add`, and the success announcement.
/// Returns the destination path (which exists after a successful `git
/// worktree add`, even if the caller's own subsequent setup step fails —
/// `Error::Setup`'s message says exactly that).
fn create_core(ctx: &RepoContext, config: &Config, req: &CreateRequest) -> Result<PathBuf> {
    let branch = req.branch;
    let branch_exists = local_branch_exists(ctx, branch)?;

    // Pre-check: refuse when some worktree already has this branch checked
    // out (git would refuse too, but with a rougher message and after we
    // may already have created directories).
    if branch_exists {
        let items = worktree::list(
            ctx,
            &ListOptions {
                with_status: false,
                base: None,
            },
        )?;
        if let Some(existing) = items.iter().find(|i| i.branch.as_deref() == Some(branch)) {
            return Err(Error::BranchInUse {
                branch: branch.to_string(),
                path: existing.path.clone(),
            });
        }
    }

    let dest = destination(ctx, config, req.path_override, branch)?;
    if dest.exists() {
        return Err(Error::DestinationExists(dest));
    }
    if req.verbose {
        eprintln!("destination: {}", dest.display());
    }

    if branch_exists {
        gitcmd::worktree_add(&ctx.main_root, &dest, branch, req.quiet)?;
    } else {
        let base = resolve_base(ctx, config, req.base_override, req.quiet)?;
        if req.verbose {
            eprintln!("creating branch '{branch}' from '{base}'");
        }
        gitcmd::worktree_add_new_branch(&ctx.main_root, &dest, branch, &base, req.quiet)?;
    }

    // Success line first: the worktree exists now, even if setup fails.
    if req.announce {
        println!("Created worktree '{branch}' at {}", dest.display());
    }

    Ok(dest)
}

/// Request the parent-shell directory change (`--cd`), after every setup
/// step has already succeeded — shared by [`create`] and [`create_streaming`].
fn finish_cd(dest: &Path, req: &CreateRequest) -> Result<()> {
    if req.cd {
        let wrote = crate::cdfile::request(dest)?;
        if !wrote && !req.quiet {
            eprintln!(
                "hint: `--cd` needs the shell wrapper; add \
                 `eval \"$(command wtm init zsh)\"` (or bash) to your shell rc"
            );
        }
    }
    Ok(())
}

/// Does a local branch with this exact name exist?
fn local_branch_exists(ctx: &RepoContext, branch: &str) -> Result<bool> {
    let repo = ctx.open_main()?;
    let found = match repo.find_branch(branch, git2::BranchType::Local) {
        Ok(_) => Ok(true),
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    };
    found
}

/// Destination directory: an explicit override (made absolute against the
/// cwd) wins, otherwise the configured path template rendered for this
/// branch.
fn destination(
    ctx: &RepoContext,
    config: &Config,
    path_override: Option<&Path>,
    branch: &str,
) -> Result<PathBuf> {
    match path_override {
        Some(p) => {
            let abs = if p.is_absolute() {
                p.to_path_buf()
            } else {
                std::env::current_dir()?.join(p)
            };
            Ok(template::normalize(&abs))
        }
        None => template::render(
            &config.path_template,
            &TemplateContext {
                repo_name: &ctx.repo_name,
                branch,
                main_root: &ctx.main_root,
            },
        ),
    }
}

/// Base ref for a new branch: override > configured `default_base`
/// (revparsed strictly in the main repo) > HEAD. An explicit base that does
/// not resolve is an error.
fn resolve_base(
    ctx: &RepoContext,
    config: &Config,
    base_override: Option<&str>,
    _quiet: bool,
) -> Result<String> {
    let requested = match base_override {
        Some(base) => Some(base),
        None => config.default_base.as_deref(),
    };
    match requested {
        None => Ok("HEAD".to_string()),
        Some(base) => {
            let repo = ctx.open_main()?;
            repo.revparse_single(base)
                .and_then(|object| object.peel_to_commit())
                .map_err(|_| {
                    Error::Other(format!(
                        "configured base '{base}' does not resolve to a commit"
                    ))
                })?;
            Ok(base.to_string())
        }
    }
}

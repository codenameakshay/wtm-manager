//! `wtm add` — create a worktree for an existing or new branch.
//!
//! The creation flow itself lives in [`create`] so that the CLI command and
//! the TUI create form share one implementation (checks, destination
//! resolution, git invocation, setup automation).

use std::path::{Path, PathBuf};

use crate::cli::{AddArgs, GlobalArgs};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::repo::RepoContext;
use crate::template::{self, TemplateContext};
use crate::worktree::{self, ListOptions};
use crate::{gitcmd, setup};

/// Everything [`create`] needs beyond repo/config. The CLI fills this from
/// `AddArgs`; the TUI fills it from its create form (with `announce`/`cd`
/// off, since it owns the terminal and never wants stdout output).
pub(crate) struct CreateRequest<'a> {
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
        announce: true,
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

/// Shared creation core: branch-in-use pre-check, destination resolution,
/// branch-exists check, `git worktree add`, optional cd-file write, and
/// post-create setup. Returns the destination path (which exists after a
/// successful `git worktree add`, even if setup fails — `Error::Setup`'s
/// message says exactly that).
pub(crate) fn create(ctx: &RepoContext, config: &Config, req: &CreateRequest) -> Result<PathBuf> {
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
        gitcmd::worktree_add(&ctx.main_root, &dest, branch)?;
    } else {
        let base = resolve_base(ctx, config, req.base_override, req.quiet)?;
        if req.verbose {
            eprintln!("creating branch '{branch}' from '{base}'");
        }
        gitcmd::worktree_add_new_branch(&ctx.main_root, &dest, branch, &base)?;
    }

    // Success line first: the worktree exists now, even if setup fails.
    if req.announce {
        println!("Created worktree '{branch}' at {}", dest.display());
    }

    if req.cd {
        // The shell wrapper installed by `wtm init` performs the actual `cd`
        // in the parent shell by reading the cd file we write here.
        let wrote = crate::cdfile::request(&dest)?;
        if !wrote && !req.quiet {
            eprintln!(
                "hint: `--cd` needs the shell wrapper; add \
                 `eval \"$(command wtm init zsh)\"` (or bash) to your shell rc"
            );
        }
    }

    if req.run_setup {
        setup::run(config, &ctx.main_root, &dest, req.quiet)?;
    }

    Ok(dest)
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
/// (revparsed in the main repo, falling back to HEAD with a stderr note when
/// it does not resolve) > HEAD.
fn resolve_base(
    ctx: &RepoContext,
    config: &Config,
    base_override: Option<&str>,
    quiet: bool,
) -> Result<String> {
    let requested = match base_override {
        Some(base) => Some(base),
        None => config.default_base.as_deref(),
    };
    match requested {
        None => Ok("HEAD".to_string()),
        Some(base) => {
            let repo = ctx.open_main()?;
            if repo.revparse_single(base).is_ok() {
                Ok(base.to_string())
            } else {
                if !quiet {
                    eprintln!("note: base '{base}' does not resolve; using HEAD instead");
                }
                Ok("HEAD".to_string())
            }
        }
    }
}

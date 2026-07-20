//! `wtm add` — create a worktree for an existing or new branch.

use std::path::PathBuf;

use crate::cli::{AddArgs, GlobalArgs};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::repo::RepoContext;
use crate::template::{self, TemplateContext};
use crate::worktree::{self, ListOptions};
use crate::{gitcmd, setup};

/// Create a worktree for `args.branch`, creating the branch when needed.
pub fn run(args: &AddArgs, global: &GlobalArgs) -> Result<()> {
    let (ctx, config) = super::prepare(global)?;
    let branch = args.branch.as_str();

    let branch_exists = local_branch_exists(&ctx, branch)?;

    // Pre-check: refuse when some worktree already has this branch checked
    // out (git would refuse too, but with a rougher message and after we
    // may already have created directories).
    if branch_exists {
        let items = worktree::list(
            &ctx,
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

    let dest = destination(&ctx, &config, args, branch)?;
    if dest.exists() {
        return Err(Error::DestinationExists(dest));
    }
    if global.verbose {
        eprintln!("destination: {}", dest.display());
    }

    if branch_exists {
        gitcmd::worktree_add(&ctx.main_root, &dest, branch)?;
    } else {
        let base = resolve_base(&ctx, &config, args, global)?;
        if global.verbose {
            eprintln!("creating branch '{branch}' from '{base}'");
        }
        gitcmd::worktree_add_new_branch(&ctx.main_root, &dest, branch, &base)?;
    }

    // Success line first: the worktree exists now, even if setup fails
    // (Error::Setup's message says exactly that), and the shell wrapper
    // reads this stream.
    println!("Created worktree '{branch}' at {}", dest.display());

    // `--cd` is deliberately a no-op here: the shell wrapper installed by
    // `wtm init` performs the actual `cd` in the parent shell.

    if !args.no_setup {
        setup::run(&config, &ctx.main_root, &dest, global.quiet)?;
    }

    if args.open {
        super::open::spawn_editor(&config, &dest)?;
        if !global.quiet {
            eprintln!("opened {} in your editor", dest.display());
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

/// Destination directory: `--path` (made absolute against the cwd) wins,
/// otherwise the configured path template rendered for this branch.
fn destination(
    ctx: &RepoContext,
    config: &Config,
    args: &AddArgs,
    branch: &str,
) -> Result<PathBuf> {
    match &args.path {
        Some(p) => {
            let abs = if p.is_absolute() {
                p.clone()
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

/// Base ref for a new branch: `--from` > configured `default_base` (revparsed
/// in the main repo, falling back to HEAD with a stderr note when it does not
/// resolve) > HEAD.
fn resolve_base(
    ctx: &RepoContext,
    config: &Config,
    args: &AddArgs,
    global: &GlobalArgs,
) -> Result<String> {
    let requested = match args.from.as_deref() {
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
                if !global.quiet {
                    eprintln!("note: base '{base}' does not resolve; using HEAD instead");
                }
                Ok("HEAD".to_string())
            }
        }
    }
}

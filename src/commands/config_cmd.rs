//! `wtm config` — inspect config file paths and scaffold a repo config.

use std::path::Path;

use crate::cli::{ConfigArgs, ConfigCommand, GlobalArgs};
use crate::config;
use crate::error::Result;
use crate::repo;

/// Dispatch `wtm config path` / `wtm config init`.
pub fn run(args: &ConfigArgs, global: &GlobalArgs) -> Result<()> {
    match args.command {
        ConfigCommand::Path => print_paths(global),
        ConfigCommand::Init => scaffold(global),
    }
}

/// Print every config path wtm consults, with an exists/missing marker.
fn print_paths(global: &GlobalArgs) -> Result<()> {
    match config::global_config_path() {
        Some(p) => println!("global: {} {}", p.display(), marker(&p)),
        None => println!("global: (no home directory found)"),
    }

    // Repo-level paths only when we are actually inside a repository.
    match repo::discover(global.repo.as_deref()) {
        Ok(ctx) => {
            let repo_cfg = ctx.main_root.join(".worktree.toml");
            let local_cfg = ctx.main_root.join(".worktree.local.toml");
            println!("repo:   {} {}", repo_cfg.display(), marker(&repo_cfg));
            println!("local:  {} {}", local_cfg.display(), marker(&local_cfg));
        }
        Err(_) => {
            if !global.quiet {
                eprintln!("note: not inside a git repository; repo-level paths not shown");
            }
        }
    }
    Ok(())
}

/// Write the commented sample `.worktree.toml` at the repository root.
fn scaffold(global: &GlobalArgs) -> Result<()> {
    let ctx = repo::discover(global.repo.as_deref())?;
    let path = config::scaffold_repo_config(&ctx.main_root)?;
    println!("Created {}", path.display());
    Ok(())
}

fn marker(path: &Path) -> &'static str {
    if path.exists() {
        "(exists)"
    } else {
        "(missing)"
    }
}

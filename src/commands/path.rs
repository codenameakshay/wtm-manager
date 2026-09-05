//! `wtm path` — print a worktree's absolute path (scripting-friendly:
//! never interactive).

use crate::cli::{GlobalArgs, PathArgs};
use crate::error::Result;
use crate::repo;
use crate::worktree;

/// Print the path of the named worktree, or the nearest Git worktree root
/// containing the current directory when no name is given (falling back to
/// the explicit repository's main worktree when the cwd is outside it).
/// Deliberately never a picker.
pub fn run(args: &PathArgs, global: &GlobalArgs) -> Result<()> {
    // No config needed here; keep startup lazy.
    let ctx = repo::discover(global.repo.as_deref())?;
    let path = match args.name.as_deref() {
        Some(name) => worktree::find(&ctx, name)?.path,
        None => {
            let cwd = std::env::current_dir()?;
            let discovered = if global.repo.is_none() {
                git2::Repository::discover(&cwd)
                    .ok()
                    .and_then(|git_repo| git_repo.workdir().map(repo::canonicalize_lossy))
            } else {
                None
            };
            match discovered {
                Some(root) => root,
                None => match worktree::containing(&ctx, &cwd)? {
                    Some(wt) => wt.path,
                    None => ctx.main_root.clone(),
                },
            }
        }
    };
    println!("{}", path.display());
    Ok(())
}

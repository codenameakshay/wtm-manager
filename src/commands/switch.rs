//! `wtm switch` — hand a worktree path to the shell wrapper to `cd` into.

use crate::cdfile;
use crate::cli::{GlobalArgs, SwitchArgs};
use crate::error::Result;
use crate::repo;

/// Resolve a worktree and request a cd into it.
///
/// When the shell wrapper is active (`$WTM_CD_FILE` set), the target path is
/// written to the cd file and the wrapper performs the actual `cd`. Without
/// the wrapper the path is printed instead, with a `wtm init` hint. The
/// hidden `--print-path` flag additionally forces the path onto stdout for
/// scripts (everything else — including the interactive picker — goes to
/// stderr).
pub fn run(args: &SwitchArgs, global: &GlobalArgs) -> Result<()> {
    // No config needed here; keep startup lazy.
    let ctx = repo::discover(global.repo.as_deref())?;
    let target = super::resolve_target(&ctx, args.name.as_deref(), "switch")?;

    let wrote = cdfile::request(&target.path)?;

    if args.print_path || !wrote {
        println!("{}", target.path.display());
    }

    if !wrote && !args.print_path && !global.quiet {
        eprintln!(
            "hint: `wtm switch` cannot change your shell's directory by itself; \
             add `eval \"$(command wtm init zsh)\"` (or bash) to your shell rc \
             to make `wtm switch` cd for you"
        );
    }
    Ok(())
}

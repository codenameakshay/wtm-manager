//! `wtm switch` — print a worktree path for the shell wrapper to `cd` into.

use crate::cli::{GlobalArgs, SwitchArgs};
use crate::error::Result;
use crate::repo;

/// Resolve a worktree and print its path.
///
/// With the hidden `--print-path` flag, stdout carries ONLY the path (the
/// shell wrapper captures it); everything else — including the interactive
/// picker — goes to stderr. Without the flag, also hint at `wtm init`.
pub fn run(args: &SwitchArgs, global: &GlobalArgs) -> Result<()> {
    // No config needed here; keep startup lazy.
    let ctx = repo::discover(global.repo.as_deref())?;
    let target = super::resolve_target(&ctx, args.name.as_deref(), "switch")?;

    println!("{}", target.path.display());

    if !args.print_path && !global.quiet {
        eprintln!(
            "hint: `wtm switch` cannot change your shell's directory by itself; \
             add `eval \"$(command wtm init zsh)\"` (or bash) to your shell rc \
             to make `wtm switch` cd for you"
        );
    }
    Ok(())
}

//! `wtm list` — enumerate worktrees as a table or JSON.

use crate::cli::{GlobalArgs, ListArgs};
use crate::error::Result;
use crate::output;
use crate::worktree::{self, ListOptions};

/// List all worktrees; status computation is skipped with `--no-status`.
pub fn run(args: &ListArgs, global: &GlobalArgs) -> Result<()> {
    let (ctx, config) = super::prepare(global)?;
    let with_status = !args.no_status;
    let items = worktree::list(
        &ctx,
        &ListOptions {
            with_status,
            base: config.default_base.clone(),
        },
    )?;

    if args.json {
        println!("{}", output::render_json(&items));
    } else {
        let color = output::use_color(global.color);
        println!("{}", output::render_table(&items, color, with_status));
    }
    Ok(())
}

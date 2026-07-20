//! `wtm completions` — generate a shell completion script on stdout.

use clap::CommandFactory;

use crate::cli::{Cli, CompletionsArgs, GlobalArgs};
use crate::error::Result;

/// Emit the completion script for the requested shell.
pub fn run(args: &CompletionsArgs, _global: &GlobalArgs) -> Result<()> {
    clap_complete::generate(
        args.shell.to_clap_shell(),
        &mut Cli::command(),
        "wtm",
        &mut std::io::stdout(),
    );
    Ok(())
}

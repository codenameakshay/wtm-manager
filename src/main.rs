//! Thin binary entrypoint: parse the CLI, dispatch, report errors.
//!
//! Usage errors are handled by clap itself (exit code 2); any command error
//! is printed as `error: ...` on stderr (red when appropriate) with exit 1.

use std::io::IsTerminal;

use clap::Parser;
use owo_colors::OwoColorize;

use wtm::cli::Cli;
use wtm::output;

fn main() {
    let cli = Cli::parse();
    if let Err(err) = wtm::commands::dispatch(&cli) {
        if matches!(err, wtm::error::Error::Cancelled) {
            return;
        }
        let message = format!("error: {err}");
        let stderr_color = output::color_enabled(cli.global.color, std::io::stderr().is_terminal());
        if stderr_color {
            eprintln!("{}", message.red());
        } else {
            eprintln!("{message}");
        }
        std::process::exit(1);
    }
}

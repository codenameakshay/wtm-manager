//! Thin binary entrypoint: parse the CLI, dispatch, report errors.
//!
//! Usage errors are handled by clap itself (exit code 2); any command error
//! is printed as `error: ...` on stderr (red when appropriate) with exit 1.

use std::io::IsTerminal;

use clap::Parser;
use owo_colors::OwoColorize;

use wtm::cli::Cli;
use wtm::output::ColorMode;

fn main() {
    let cli = Cli::parse();
    if let Err(err) = wtm::commands::dispatch(&cli) {
        if matches!(err, wtm::error::Error::Cancelled) {
            return;
        }
        let message = format!("error: {err}");
        if stderr_color(cli.global.color) {
            eprintln!("{}", message.red());
        } else {
            eprintln!("{message}");
        }
        std::process::exit(1);
    }
}

/// Color on stderr: `--color` wins; on auto, require a stderr TTY and an
/// unset/empty `NO_COLOR`.
fn stderr_color(mode: ColorMode) -> bool {
    match mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => {
            std::io::stderr().is_terminal()
                && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        }
    }
}

//! Command implementations. One module per subcommand, plus shared helpers
//! for repository/config resolution and the interactive picker.

pub mod add;
pub mod app;
pub mod completions;
pub mod config_cmd;
pub mod init;
pub mod list;
pub mod open;
pub mod path;
pub mod prune;
pub mod remove;
pub mod switch;

use std::io::{IsTerminal, Write};

use crate::cli::{Cli, Command, GlobalArgs};
use crate::config::{self, Config};
use crate::error::{Error, Result};
use crate::model::WorktreeInfo;
use crate::repo::{self, RepoContext};
use crate::worktree::{self, ListOptions};

/// Dispatch a parsed CLI invocation to its command implementation.
pub fn dispatch(cli: &Cli) -> Result<()> {
    let Some(command) = &cli.command else {
        return bare(&cli.global);
    };
    match command {
        Command::Add(args) => add::run(args, &cli.global),
        Command::List(args) => list::run(args, &cli.global),
        Command::Remove(args) => remove::run(args, &cli.global),
        Command::Switch(args) => switch::run(args, &cli.global),
        Command::Prune(args) => prune::run(args, &cli.global),
        Command::Open(args) => open::run(args, &cli.global),
        Command::Path(args) => path::run(args, &cli.global),
        Command::App => launch_app(&cli.global),
        Command::Tui => crate::tui::run(&cli.global),
        Command::Init(args) => init::run(args, &cli.global),
        Command::Completions(args) => completions::run(args, &cli.global),
        Command::Config(args) => config_cmd::run(args, &cli.global),
    }
}

/// Bare `wtm`: on a terminal, open the desktop app — falling back to the TUI
/// when the app is not installed. In a pipe/agent/CI context, print help and
/// exit 0: those callers must never get a full-screen UI, and opening a window
/// out of a script would be just as surprising.
fn bare(global: &GlobalArgs) -> Result<()> {
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        use clap::CommandFactory;
        Cli::command().print_help()?;
        return Ok(());
    }

    match app::try_launch(global) {
        Ok(true) => Ok(()),
        // No app installed: the TUI is the interactive fallback.
        Ok(false) => crate::tui::run(global),
        // The app exists but would not start. Say so, then still give the
        // user something usable rather than nothing.
        Err(e) => {
            eprintln!("warning: could not launch the wtm app: {e}");
            crate::tui::run(global)
        }
    }
}

/// `wtm app`: explicitly open the desktop app, whatever the terminal context.
fn launch_app(global: &GlobalArgs) -> Result<()> {
    if app::try_launch(global)? {
        return Ok(());
    }
    Err(Error::Other(
        "the wtm desktop app is not installed (expected WTM.app in /Applications \
         or a `wtm-gui` binary on PATH); run `wtm tui` for the terminal UI"
            .to_string(),
    ))
}

/// Resolve the repository context (honoring `-C/--repo`) and load the layered
/// configuration for it. Shared by every command that needs both.
pub(crate) fn prepare(global: &GlobalArgs) -> Result<(RepoContext, Config)> {
    let ctx = repo::discover(global.repo.as_deref())?;
    let config = config::load(&ctx.main_root)?;
    Ok((ctx, config))
}

/// Resolve a worktree from an optional name: exact/branch/substring match via
/// [`worktree::find`] when a name was given, otherwise the interactive picker
/// (TTY-gated). `command` names the invoking subcommand for error messages.
pub(crate) fn resolve_target(
    ctx: &RepoContext,
    name: Option<&str>,
    command: &str,
) -> Result<WorktreeInfo> {
    match name {
        Some(n) => worktree::find(ctx, n),
        None => {
            let items = worktree::list(
                ctx,
                &ListOptions {
                    with_status: false,
                    base: None,
                },
            )?;
            pick(&items, command)
        }
    }
}

/// Interactive fuzzy picker over worktree display names.
///
/// Allowed only when BOTH stdin and stderr are terminals; stdout is
/// deliberately not required to be one (the shell wrapper captures stdout).
/// While the prompt runs, fd 1 is swapped to fd 2 via an RAII guard so any
/// output inquire writes to stdout lands on stderr and captured stdout stays
/// clean.
pub(crate) fn pick(items: &[WorktreeInfo], command: &str) -> Result<WorktreeInfo> {
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        return Err(Error::NotATty(command.to_string()));
    }
    if items.is_empty() {
        return Err(Error::Other("no worktrees found".to_string()));
    }

    let names: Vec<String> = items.iter().map(|i| i.display_name().to_string()).collect();

    let selection = {
        let _guard = StdoutToStderrGuard::new()?;
        inquire::Select::new(&format!("Select a worktree to {command}:"), names).raw_prompt()
    };

    match selection {
        Ok(choice) => Ok(items[choice.index].clone()),
        Err(inquire::InquireError::OperationCanceled)
        | Err(inquire::InquireError::OperationInterrupted) => Err(Error::Cancelled),
        Err(e) => Err(Error::Other(format!("selection failed: {e}"))),
    }
}

/// RAII guard that redirects fd 1 (stdout) to fd 2 (stderr) for its lifetime.
///
/// `dup(1)` saves the real stdout, `dup2(2, 1)` points fd 1 at stderr, and
/// `Drop` restores + closes the saved descriptor — including on early return
/// or error, which is the point of doing this via RAII.
struct StdoutToStderrGuard {
    saved_stdout: libc::c_int,
}

impl StdoutToStderrGuard {
    fn new() -> Result<Self> {
        // Flush Rust-level buffers before touching the underlying fd.
        let _ = std::io::stdout().flush();
        // SAFETY: plain fd duplication; we check the return values.
        let saved_stdout = unsafe { libc::dup(1) };
        if saved_stdout < 0 {
            return Err(Error::Io(std::io::Error::last_os_error()));
        }
        if unsafe { libc::dup2(2, 1) } < 0 {
            let err = std::io::Error::last_os_error();
            unsafe { libc::close(saved_stdout) };
            return Err(Error::Io(err));
        }
        Ok(Self { saved_stdout })
    }
}

impl Drop for StdoutToStderrGuard {
    fn drop(&mut self) {
        // Flush anything the prompt buffered for "stdout" (now stderr).
        let _ = std::io::stdout().flush();
        // SAFETY: restoring the descriptor we saved in `new`.
        unsafe {
            libc::dup2(self.saved_stdout, 1);
            libc::close(self.saved_stdout);
        }
    }
}

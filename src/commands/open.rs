//! `wtm open` — open a worktree in the editor, or run a command inside it.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::cli::{GlobalArgs, OpenArgs};
use crate::config::Config;
use crate::error::{Error, Result};

/// Open a worktree (picked interactively when no name is given).
pub fn run(args: &OpenArgs, global: &GlobalArgs) -> Result<()> {
    let (ctx, config) = super::prepare(global)?;
    let target = super::resolve_target(&ctx, args.name.as_deref(), "open")?;

    match &args.with {
        Some(cmd) => run_command_in(cmd, &target.path),
        None => {
            spawn_editor(&config, &target.path)?;
            if !global.quiet {
                eprintln!("opened {} in your editor", target.path.display());
            }
            Ok(())
        }
    }
}

/// Run `cmd` via `sh -c` with the worktree as cwd, streaming output, and
/// propagate a non-zero exit as an error.
fn run_command_in(cmd: &str, worktree: &Path) -> Result<()> {
    let status = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(worktree)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Other(format!(
            "command `{cmd}` failed with {status}"
        )))
    }
}

/// Launch the editor on `path` without waiting for it to exit.
///
/// Editor resolution order: config `editor` > `$VISUAL` > `$EDITOR`. The
/// editor value may contain arguments, so it runs through `sh -c` with the
/// path passed safely as `$0`.
pub(crate) fn spawn_editor(config: &Config, path: &Path) -> Result<()> {
    let editor = config
        .editor
        .clone()
        .or_else(|| env_nonempty("VISUAL"))
        .or_else(|| env_nonempty("EDITOR"))
        .ok_or_else(|| {
            Error::Config(
                "no editor configured (set `editor` in your wtm config, or export $VISUAL/$EDITOR)"
                    .to_string(),
            )
        })?;

    // `sh -c '<editor> "$0"' <path>` keeps paths with spaces intact without
    // hand-rolled quoting.
    Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$0\""))
        .arg(path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    Ok(())
}

/// A non-empty environment variable, if set.
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

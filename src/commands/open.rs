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

    let program = first_shell_word(&editor).ok_or_else(|| {
        Error::Config("editor command is empty or has invalid quoting".to_string())
    })?;
    // Literal commands can be checked without running them. Rich shell forms
    // (environment assignments, `$HOME`, `~`, command substitution) are left
    // to the same shell that launches them so valid expansion is not rejected.
    if is_literal_program(&program) {
        let preflight = Command::new("sh")
            .arg("-c")
            .arg("command -v -- \"$1\" >/dev/null 2>&1")
            .arg("wtm-editor-check")
            .arg(&program)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !preflight.success() {
            return Err(Error::Config(format!(
                "editor command is not available: {editor}"
            )));
        }
    }

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

fn is_literal_program(program: &str) -> bool {
    let assignment = program.split_once('=').is_some_and(|(name, _)| {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
            && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    });
    !assignment && !program.starts_with('~') && !program.contains('$') && !program.contains('`')
}

/// Extract the executable token without executing shell expansions. This
/// covers ordinary commands and quoted executable paths while keeping the
/// availability check side-effect free.
fn first_shell_word(command: &str) -> Option<String> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut quote = Quote::None;
    let mut escaped = false;
    let mut started = false;
    let mut word = String::new();
    for ch in command.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
            started = true;
            continue;
        }
        match quote {
            Quote::None => match ch {
                '\\' => {
                    escaped = true;
                    started = true;
                }
                '\'' => {
                    quote = Quote::Single;
                    started = true;
                }
                '"' => {
                    quote = Quote::Double;
                    started = true;
                }
                ch if ch.is_whitespace() => {
                    if started {
                        break;
                    }
                }
                _ => {
                    word.push(ch);
                    started = true;
                }
            },
            Quote::Single => {
                if ch == '\'' {
                    quote = Quote::None;
                } else {
                    word.push(ch);
                }
            }
            Quote::Double => match ch {
                '"' => quote = Quote::None,
                '\\' => escaped = true,
                _ => word.push(ch),
            },
        }
    }
    if escaped || quote != Quote::None || word.is_empty() {
        None
    } else {
        Some(word)
    }
}

/// A non-empty environment variable, if set.
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::{first_shell_word, is_literal_program};

    #[test]
    fn editor_program_supports_arguments_and_quoted_paths() {
        assert_eq!(first_shell_word("code --wait").as_deref(), Some("code"));
        assert_eq!(
            first_shell_word("'/Applications/Visual Studio Code.app/bin/code' --wait").as_deref(),
            Some("/Applications/Visual Studio Code.app/bin/code")
        );
        assert_eq!(first_shell_word("  "), None);
        assert_eq!(first_shell_word("'unterminated"), None);
        assert!(is_literal_program("code"));
        assert!(is_literal_program(
            "/Applications/Visual Studio Code.app/bin/code"
        ));
        assert!(!is_literal_program("~/bin/editor"));
        assert!(!is_literal_program("$HOME/bin/editor"));
        assert!(!is_literal_program("TERM=xterm-256color"));
    }
}

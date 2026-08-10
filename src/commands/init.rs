//! `wtm init` — print shell integration: the `wtm` wrapper function (which
//! implements cd-on-exit for `switch`, `add --cd`, and the TUI via a temp
//! cd file) plus completion loading for the chosen shell.

use crate::cli::{GlobalArgs, InitArgs, ShellKind};
use crate::error::Result;

/// The wrapper function. Valid in both zsh and bash.
///
/// It creates a temp file, exports its path as `$WTM_CD_FILE`, and cd's into
/// whatever path the binary wrote there. Unlike stdout capture, this works
/// for the full-screen TUI too (which owns the terminal).
const WRAPPER: &str = r#"wtm() {
  local cdfile; cdfile="$(mktemp -t wtm-cd.XXXXXX)" || return
  WTM_CD_FILE="$cdfile" command wtm "$@"; local status=$?
  if [ "$status" -eq 0 ] && [ -s "$cdfile" ]; then
    local target; target="$(cat "$cdfile")"; target="${target%.}"
    builtin cd -- "$target" || status=$?
  fi
  rm -f "$cdfile"; return $status
}"#;

/// Completion loading for zsh. `eval "$(wtm completions zsh)"` alone does not
/// work: the script is a `#compdef` file that must be discovered via `$fpath`
/// (or registered with `compdef` after compinit). So: write it to a cache
/// file, put that directory on `$fpath`, and if compinit already ran,
/// register the function explicitly.
const ZSH_COMPLETIONS: &str = r#"# Completions: cache the script and register it with the completion system.
_wtm_comp_dir="${XDG_CACHE_HOME:-$HOME/.cache}/wtm"
if command mkdir -p "$_wtm_comp_dir" 2>/dev/null; then
  command wtm completions zsh >| "$_wtm_comp_dir/_wtm" 2>/dev/null
  fpath=("$_wtm_comp_dir" $fpath)
  if (( ${+functions[compdef]} )); then
    # compinit already ran; (re)load the freshly written completion now.
    unfunction _wtm 2>/dev/null
    autoload -Uz _wtm
    compdef _wtm wtm
  fi
  # Otherwise compinit will pick _wtm up from fpath when it runs.
fi
unset _wtm_comp_dir"#;

/// Completion loading for bash: the generated script is directly evalable.
const BASH_COMPLETIONS: &str = r#"# Completions.
eval "$(command wtm completions bash)""#;

/// Print the shell wrapper and completion setup for `args.shell`.
pub fn run(args: &InitArgs, _global: &GlobalArgs) -> Result<()> {
    match args.shell {
        ShellKind::Zsh => {
            println!("# wtm shell integration (zsh).");
            println!("# Add to ~/.zshrc:  eval \"$(command wtm init zsh)\"");
            println!("{WRAPPER}");
            println!();
            println!("{ZSH_COMPLETIONS}");
        }
        ShellKind::Bash => {
            println!("# wtm shell integration (bash).");
            println!("# Add to ~/.bashrc:  eval \"$(command wtm init bash)\"");
            println!("{WRAPPER}");
            println!();
            println!("{BASH_COMPLETIONS}");
        }
    }
    Ok(())
}

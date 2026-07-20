//! `wtm init` — print shell integration: the `wtm` wrapper function (which
//! implements `switch`/`--cd` by cd-ing in the parent shell) plus completion
//! loading for the chosen shell.

use crate::cli::{GlobalArgs, InitArgs, ShellKind};
use crate::error::Result;

/// The wrapper function, byte-for-byte as specified in DESIGN.md. Valid in
/// both zsh and bash.
const WRAPPER: &str = r#"wtm() {
  case "$1" in
    switch|cd|sw)
      shift
      local d
      d="$(command wtm switch --print-path "$@")" || return
      [ -n "$d" ] && builtin cd "$d" ;;
    add|new|create)
      command wtm "$@" || return
      case " $* " in
        *" --cd "*)
          local b="" a
          for a in "${@:2}"; do case "$a" in -*) ;; *) b="$a"; break ;; esac; done
          if [ -n "$b" ]; then
            local d
            d="$(command wtm path "$b")" && [ -n "$d" ] && builtin cd "$d"
          fi ;;
      esac ;;
    *) command wtm "$@" ;;
  esac
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

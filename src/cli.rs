//! Command-line interface definitions (clap v4 derive).
//!
//! This module only *declares* the CLI surface; all behavior lives in
//! [`crate::commands`].

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::output::ColorMode;

/// A fast, ergonomic Git worktree manager.
#[derive(Debug, Parser)]
#[command(name = "wtm", version, about, propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    #[command(flatten)]
    pub global: GlobalArgs,
}

/// Options accepted by every subcommand.
#[derive(Debug, Clone, Args)]
pub struct GlobalArgs {
    /// Operate on the repository at this path instead of the current directory.
    #[arg(short = 'C', long = "repo", global = true, value_name = "PATH")]
    pub repo: Option<PathBuf>,

    /// When to use colored output.
    #[arg(long, global = true, value_enum, default_value_t = ColorMode::Auto)]
    pub color: ColorMode,

    /// Print extra detail about what is happening.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Suppress non-essential informational messages.
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,
}

/// All `wtm` subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a new worktree for a branch (creating the branch if needed).
    #[command(visible_aliases = ["new", "create"])]
    Add(AddArgs),

    /// List all worktrees with their status.
    #[command(visible_alias = "ls")]
    List(ListArgs),

    /// Remove a worktree (and optionally its branch).
    #[command(visible_alias = "rm")]
    Remove(RemoveArgs),

    /// Print a worktree's path so the shell wrapper can cd into it.
    #[command(visible_aliases = ["cd", "sw"])]
    Switch(SwitchArgs),

    /// Remove stale worktrees: missing directories, and optionally
    /// merged or upstream-gone branches.
    #[command(visible_alias = "clean")]
    Prune(PruneArgs),

    /// Open a worktree in your editor (or run an arbitrary command in it).
    Open(OpenArgs),

    /// Print the absolute path of a worktree (scripting-friendly).
    Path(PathArgs),

    /// Print shell integration (the `wtm` wrapper function plus completions).
    Init(InitArgs),

    /// Generate shell completions for wtm.
    Completions(CompletionsArgs),

    /// Inspect or scaffold wtm configuration files.
    Config(ConfigArgs),
}

/// Arguments for `wtm add`.
#[derive(Debug, Clone, Args)]
pub struct AddArgs {
    /// Branch to check out in the new worktree (created from the base ref
    /// when it does not exist yet).
    #[arg(value_name = "BRANCH")]
    pub branch: String,

    /// Base ref for a newly created branch (overrides the configured
    /// `default_base`; falls back to HEAD when unresolvable).
    #[arg(long, value_name = "BASE")]
    pub from: Option<String>,

    /// Destination directory for the worktree (overrides the configured
    /// path template).
    #[arg(long, value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Change into the new worktree after creation (handled by the shell
    /// wrapper installed via `wtm init`).
    #[arg(long)]
    pub cd: bool,

    /// Open the new worktree in your editor after creation.
    #[arg(long)]
    pub open: bool,

    /// Skip the configured post-create setup (copy entries and commands).
    #[arg(long)]
    pub no_setup: bool,
}

/// Arguments for `wtm list`.
#[derive(Debug, Clone, Args)]
pub struct ListArgs {
    /// Emit the worktree list as pretty-printed JSON.
    #[arg(long)]
    pub json: bool,

    /// Skip per-worktree status computation (dirty/ahead/behind/merged);
    /// much faster on large repositories.
    #[arg(long, visible_alias = "fast")]
    pub no_status: bool,
}

/// Arguments for `wtm remove`.
#[derive(Debug, Clone, Args)]
pub struct RemoveArgs {
    /// Worktree to remove (branch name, registry name, or unique substring).
    /// Omit to pick interactively.
    #[arg(value_name = "NAME")]
    pub name: Option<String>,

    /// Remove even if the worktree has uncommitted changes.
    #[arg(long)]
    pub force: bool,

    /// Also delete the worktree's branch after removal (protected branches
    /// are never deleted).
    #[arg(long)]
    pub with_branch: bool,
}

/// Arguments for `wtm switch`.
#[derive(Debug, Clone, Args)]
pub struct SwitchArgs {
    /// Worktree to switch to (branch name, registry name, or unique
    /// substring). Omit to pick interactively.
    #[arg(value_name = "NAME")]
    pub name: Option<String>,

    /// Print only the worktree path on stdout (used by the shell wrapper;
    /// every other message goes to stderr).
    #[arg(long, hide = true)]
    pub print_path: bool,
}

/// Arguments for `wtm prune`.
#[derive(Debug, Clone, Args)]
pub struct PruneArgs {
    /// Also prune worktrees whose branch is merged into the base ref.
    #[arg(long)]
    pub merged: bool,

    /// Also prune worktrees whose branch's upstream no longer exists
    /// (e.g. deleted on the remote after a merged PR).
    #[arg(long)]
    pub gone: bool,

    /// Show what would be pruned without touching anything.
    #[arg(long)]
    pub dry_run: bool,

    /// Prune even worktrees with uncommitted changes.
    #[arg(long)]
    pub force: bool,
}

/// Arguments for `wtm open`.
#[derive(Debug, Clone, Args)]
pub struct OpenArgs {
    /// Worktree to open (branch name, registry name, or unique substring).
    /// Omit to pick interactively.
    #[arg(value_name = "NAME")]
    pub name: Option<String>,

    /// Run this command (via `sh -c`) inside the worktree instead of
    /// launching the editor.
    #[arg(long, value_name = "CMD")]
    pub with: Option<String>,
}

/// Arguments for `wtm path`.
#[derive(Debug, Clone, Args)]
pub struct PathArgs {
    /// Worktree to resolve (branch name, registry name, or unique
    /// substring). Omit to print the current worktree's path.
    #[arg(value_name = "NAME")]
    pub name: Option<String>,
}

/// Arguments for `wtm init`.
#[derive(Debug, Clone, Args)]
pub struct InitArgs {
    /// Shell to emit integration code for.
    #[arg(value_enum, value_name = "SHELL")]
    pub shell: ShellKind,
}

/// Arguments for `wtm completions`.
#[derive(Debug, Clone, Args)]
pub struct CompletionsArgs {
    /// Shell to generate a completion script for.
    #[arg(value_enum, value_name = "SHELL")]
    pub shell: ShellKind,
}

/// Shells supported by `wtm init` / `wtm completions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ShellKind {
    /// Z shell.
    Zsh,
    /// GNU Bourne-Again Shell.
    Bash,
}

impl ShellKind {
    /// The matching `clap_complete` shell for completion generation.
    pub fn to_clap_shell(self) -> clap_complete::Shell {
        match self {
            ShellKind::Zsh => clap_complete::Shell::Zsh,
            ShellKind::Bash => clap_complete::Shell::Bash,
        }
    }
}

/// Arguments for `wtm config`.
#[derive(Debug, Clone, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

/// Subcommands of `wtm config`.
#[derive(Debug, Clone, Subcommand)]
pub enum ConfigCommand {
    /// Print the configuration file paths wtm reads, with existence markers.
    Path,
    /// Write a commented sample `.worktree.toml` at the repository root.
    Init,
}

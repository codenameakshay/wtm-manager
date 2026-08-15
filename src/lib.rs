//! wtm — a fast, ergonomic Git worktree manager.
//!
//! All logic lives in this library; `src/main.rs` is a thin entrypoint.
//! Reads go through `git2` (never a spawned `git` process); mutations shell
//! out to the user's `git` binary so hooks and behavior match exactly.

pub mod cdfile;
pub mod cli;
pub mod commands;
pub mod config;
pub mod error;
pub mod gitcmd;
pub mod model;
pub mod output;
pub mod registry;
pub mod repo;
pub mod setup;
pub mod template;
pub mod tui;
pub mod worktree;

pub use error::{Error, Result};
pub use model::{WorktreeInfo, WorktreeStatus};

use std::path::PathBuf;

/// Library-level error type. `main` formats these as `error: {err}` and
/// exits 1.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("operation cancelled")]
    Cancelled,

    #[error("not inside a git repository (searched from {0})")]
    RepoNotFound(PathBuf),

    #[error("bare repositories without a working tree are not supported")]
    BareRepo,

    #[error("no worktree or branch named '{0}' was found")]
    WorktreeNotFound(String),

    #[error("branch '{branch}' is already checked out at {path}")]
    BranchInUse { branch: String, path: PathBuf },

    #[error("worktree '{name}' at {path} has uncommitted changes (use --force to override)")]
    Dirty { name: String, path: PathBuf },

    #[error("branch '{0}' is protected and will not be touched")]
    ProtectedBranch(String),

    #[error("destination already exists: {0}")]
    DestinationExists(PathBuf),

    #[error("{0} requires a name argument when not attached to a terminal")]
    NotATty(String),

    #[error("cannot {action} the main worktree")]
    MainWorktree { action: String },

    #[error("invalid path template: {0}")]
    Template(String),

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("git {args} failed with {status}:\n{stderr}")]
    GitCommand {
        args: String,
        status: String,
        stderr: String,
    },

    #[error("setup step failed: {0} (the worktree was created successfully; fix the issue and re-run the setup commands manually, or remove the worktree with `wtm rm`)")]
    Setup(String),

    #[error(transparent)]
    Git2(#[from] git2::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

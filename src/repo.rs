//! Repository discovery.
//!
//! Resolves the MAIN working tree from any starting directory — including
//! from inside a linked worktree or any subdirectory of one. All returned
//! paths are canonicalized so that e.g. macOS `/tmp` and `/private/tmp`
//! compare equal.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Resolved repository context. Holds paths only (git2::Repository is not
/// Sync; callers open repositories on demand, per thread).
#[derive(Debug, Clone)]
pub struct RepoContext {
    /// Absolute path to the MAIN working tree root (not a linked worktree).
    pub main_root: PathBuf,
    /// Absolute path to the main repository's .git directory (common dir).
    pub git_dir: PathBuf,
    /// Directory name of the main working tree (used as {repo} in templates).
    pub repo_name: String,
}

impl RepoContext {
    /// Open a git2 Repository for the main working tree.
    pub fn open_main(&self) -> Result<git2::Repository> {
        Ok(git2::Repository::open(&self.main_root)?)
    }
}

/// Discover the repository from `start` (or the current directory), resolving
/// to the MAIN working tree even when invoked from inside a linked worktree or
/// any subdirectory. Errors: RepoNotFound, BareRepo.
pub fn discover(start: Option<&Path>) -> Result<RepoContext> {
    let start_path = match start {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?,
    };

    let repo = git2::Repository::discover(&start_path)
        .map_err(|_| Error::RepoNotFound(start_path.clone()))?;

    let (main_root, git_dir) = if repo.is_worktree() {
        // For a linked worktree, the common dir is the main repository's
        // .git directory; the main working tree root is its parent.
        let git_dir = canonicalize_lossy(repo.commondir());
        let main_root = git_dir.parent().ok_or(Error::BareRepo)?.to_path_buf();
        (main_root, git_dir)
    } else {
        let workdir = repo.workdir().ok_or(Error::BareRepo)?;
        (
            canonicalize_lossy(workdir),
            canonicalize_lossy(repo.commondir()),
        )
    };

    let repo_name = main_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());

    Ok(RepoContext {
        main_root,
        git_dir,
        repo_name,
    })
}

/// Canonicalize when possible (resolves macOS `/tmp` vs `/private/tmp`,
/// symlinks, and `..` segments); returns the path unchanged when the file
/// does not exist or canonicalization fails.
pub fn canonicalize_lossy(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testgit::{git, init_repo};
    use std::fs;

    #[test]
    fn discovers_from_main_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let main = tmp.path().join("main");
        init_repo(&main);

        let ctx = discover(Some(&main)).unwrap();
        let canon = fs::canonicalize(&main).unwrap();
        assert_eq!(ctx.main_root, canon);
        assert_eq!(ctx.git_dir, fs::canonicalize(main.join(".git")).unwrap());
        assert_eq!(ctx.repo_name, "main");
        ctx.open_main().unwrap();
    }

    #[test]
    fn discovers_from_subdirectory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let main = tmp.path().join("main");
        init_repo(&main);
        let sub = main.join("a").join("b");
        fs::create_dir_all(&sub).unwrap();

        let ctx = discover(Some(&sub)).unwrap();
        assert_eq!(ctx.main_root, fs::canonicalize(&main).unwrap());
    }

    #[test]
    fn discovers_main_from_linked_worktree() {
        let tmp = tempfile::TempDir::new().unwrap();
        let main = tmp.path().join("main");
        init_repo(&main);
        let wt = tmp.path().join("wts").join("feat");
        git(
            &main,
            &["worktree", "add", "-b", "feat", wt.to_str().unwrap()],
        );

        // From the worktree root.
        let ctx = discover(Some(&wt)).unwrap();
        assert_eq!(ctx.main_root, fs::canonicalize(&main).unwrap());
        assert_eq!(ctx.repo_name, "main");
        assert_eq!(ctx.git_dir, fs::canonicalize(main.join(".git")).unwrap());

        // From a subdirectory inside the worktree.
        let sub = wt.join("nested").join("dir");
        fs::create_dir_all(&sub).unwrap();
        let ctx = discover(Some(&sub)).unwrap();
        assert_eq!(ctx.main_root, fs::canonicalize(&main).unwrap());
    }

    #[test]
    fn bare_repository_is_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let bare = tmp.path().join("bare.git");
        fs::create_dir(&bare).unwrap();
        git(&bare, &["init", "--bare"]);

        let err = discover(Some(&bare)).unwrap_err();
        assert!(matches!(err, Error::BareRepo), "got: {err}");
    }

    #[test]
    fn missing_repository_is_reported() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = discover(Some(tmp.path())).unwrap_err();
        assert!(matches!(err, Error::RepoNotFound(_)), "got: {err}");
    }
}

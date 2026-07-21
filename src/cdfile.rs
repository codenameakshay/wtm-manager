//! cd-on-exit mechanism shared by `wtm switch`, `wtm add --cd`, and the TUI.
//!
//! The shell wrapper installed by `wtm init` creates a temp file, exports its
//! path as `$WTM_CD_FILE`, runs the real binary, and cd's into whatever path
//! the binary wrote there. This works uniformly for plain commands and for
//! the full-screen TUI (which owns the terminal, so the old stdout-capture
//! trick cannot).

use std::path::{Path, PathBuf};

use crate::error::Result;

/// Environment variable the shell wrapper sets to the cd file's path.
pub const ENV_VAR: &str = "WTM_CD_FILE";

/// The cd file path from the environment, when the shell wrapper is active.
pub fn cd_file() -> Option<PathBuf> {
    std::env::var_os(ENV_VAR)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Record `path` as the directory the parent shell should cd into.
///
/// Returns `true` when the wrapper is active and the path was written to the
/// cd file, `false` when no `$WTM_CD_FILE` is set (callers fall back to
/// printing the path, plus a `wtm init` hint where appropriate).
pub fn request(path: &Path) -> Result<bool> {
    match cd_file() {
        Some(file) => {
            std::fs::write(&file, format!("{}\n", path.display()))?;
            Ok(true)
        }
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Env-mutating tests share the config test lock convention: keep them in
    // one test so they cannot race each other.
    #[test]
    fn request_writes_cd_file_when_env_set_and_reports_absence() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("cdfile");

        std::env::set_var(ENV_VAR, &file);
        assert_eq!(cd_file(), Some(file.clone()));
        assert!(request(Path::new("/some/where")).unwrap());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "/some/where\n",
            "cd file must hold exactly the target path"
        );

        std::env::set_var(ENV_VAR, "");
        assert_eq!(cd_file(), None, "empty env var counts as unset");

        std::env::remove_var(ENV_VAR);
        assert_eq!(cd_file(), None);
        assert!(!request(Path::new("/some/where")).unwrap());
    }
}

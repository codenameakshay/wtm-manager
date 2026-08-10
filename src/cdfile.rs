//! cd-on-exit mechanism shared by `wtm switch`, `wtm add --cd`, and the TUI.
//!
//! The shell wrapper installed by `wtm init` creates a temp file, exports its
//! path as `$WTM_CD_FILE`, runs the real binary, and cd's into whatever path
//! the binary wrote there. This works uniformly for plain commands and for
//! the full-screen TUI (which owns the terminal, so the old stdout-capture
//! trick cannot).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

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
            let name_is_private = file
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("wtm-cd."));
            let metadata = std::fs::symlink_metadata(&file)?;
            let inside_temp = std::fs::canonicalize(&file)
                .ok()
                .zip(std::fs::canonicalize(std::env::temp_dir()).ok())
                .is_some_and(|(file, temp)| file.starts_with(temp));
            if !name_is_private || !inside_temp || !metadata.file_type().is_file() {
                return Err(Error::Other(format!(
                    "refusing unsafe {ENV_VAR} target: {}",
                    file.display()
                )));
            }

            let mut options = OpenOptions::new();
            options.write(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW);
            }
            let mut output = options.open(&file)?;
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                output.write_all(path.as_os_str().as_bytes())?;
            }
            #[cfg(not(unix))]
            output.write_all(path.to_string_lossy().as_bytes())?;
            // The sentinel prevents command substitution from stripping
            // newline bytes that legitimately belong to the path.
            output.write_all(b".")?;
            Ok(true)
        }
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn request_writes_cd_file_when_env_set_and_reports_absence() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("wtm-cd.test");
        std::fs::write(&file, "").unwrap();

        std::env::set_var(ENV_VAR, &file);
        assert_eq!(cd_file(), Some(file.clone()));
        assert!(request(Path::new("/some/where")).unwrap());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "/some/where.",
            "cd file must hold the target path plus its sentinel"
        );

        std::env::set_var(ENV_VAR, "");
        assert_eq!(cd_file(), None, "empty env var counts as unset");

        std::env::remove_var(ENV_VAR);
        assert_eq!(cd_file(), None);
        assert!(!request(Path::new("/some/where")).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn request_preserves_non_utf8_and_newline_path_bytes() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("wtm-cd.bytes");
        std::fs::write(&file, "").unwrap();
        std::env::set_var(ENV_VAR, &file);
        let path = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/a\n\xff".to_vec()));
        request(&path).unwrap();

        let bytes = std::fs::read(&file).unwrap();
        assert_eq!(&bytes[..bytes.len() - 1], path.as_os_str().as_bytes());
        assert_eq!(bytes.last(), Some(&b'.'));
        std::env::remove_var(ENV_VAR);
    }
}

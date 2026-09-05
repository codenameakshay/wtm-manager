//! Launching the desktop app from the CLI.
//!
//! The GUI ships as a separate binary (`wtm-gui`) so the CLI stays small and
//! starts instantly. This module finds that binary and hands it a repository
//! to open. On macOS it is normally packaged inside a `WTM.app` bundle and
//! started via `open -a`; on Linux there is no bundle concept, so `wtm-gui`
//! is looked for as a plain executable in the handful of places a Linux
//! install or a local build actually puts it (see `locate`).
//!
//! Bare `wtm` prefers the app, because that is what most people want once it
//! is installed. Everything degrades honestly: no app installed falls back to
//! the TUI on a terminal, and to `--help` when there is no terminal at all
//! (an agent, a pipe, or CI must never end up in a full-screen UI).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::cli::GlobalArgs;
#[cfg(target_os = "macos")]
use crate::error::Error;
use crate::error::Result;

/// How to start the desktop app.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AppLauncher {
    /// A macOS application bundle, started via `open -a`. `open` returns as
    /// soon as the app is launched or focused, which is exactly the behavior
    /// wanted from a terminal.
    #[cfg(target_os = "macos")]
    Bundle(PathBuf),
    /// A plain executable (a development build, or an installed binary),
    /// spawned directly and detached from this process.
    Binary(PathBuf),
}

impl AppLauncher {
    /// Start the app, asking it to open `repo` when one is given. Returns once
    /// the app has been started; this process does not wait for it to exit.
    fn launch(&self, repo: Option<&Path>) -> Result<()> {
        let mut command = match self {
            #[cfg(target_os = "macos")]
            AppLauncher::Bundle(path) => {
                let mut c = Command::new("open");
                c.arg("-a").arg(path);
                if let Some(repo) = repo {
                    c.arg("--args").arg(repo);
                }
                c
            }
            AppLauncher::Binary(path) => {
                let mut c = Command::new(path);
                if let Some(repo) = repo {
                    c.arg(repo);
                }
                c
            }
        };

        // Detach: the window outlives the shell command that opened it, and
        // the app's output must never mix into the terminal's stdout.
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        match self {
            // `open` exits immediately; surfacing its failure is useful.
            #[cfg(target_os = "macos")]
            AppLauncher::Bundle(path) => {
                let status = command.status()?;
                if !status.success() {
                    return Err(Error::Other(format!(
                        "could not open {}: {status}",
                        path.display()
                    )));
                }
            }
            AppLauncher::Binary(_) => {
                command.spawn()?;
            }
        }
        Ok(())
    }
}

/// Find the installed desktop app, if there is one.
///
/// Search order, most specific first:
/// 1. `$WTM_APP` — an explicit path to a bundle or binary (development and
///    tests).
/// 2. macOS only: `WTM.app` in the standard install locations
///    (`bundle_candidates`).
/// 3. A `wtm-gui` binary next to the running `wtm` (a cargo target directory,
///    or a shared install prefix) -- the same on every platform.
/// 4. Linux only: `wtm-gui` in the fixed locations a Linux install actually
///    uses (`linux_binary_candidates`) -- there is no bundle-relative
///    layout to derive this from the way `bundle_candidates` does for step
///    2, so these are just the conventional per-user and system bin dirs.
/// 5. `wtm-gui` anywhere on `$PATH`.
fn locate() -> Option<AppLauncher> {
    if let Some(explicit) = std::env::var_os("WTM_APP") {
        if !explicit.is_empty() {
            let path = PathBuf::from(explicit);
            return classify(&path);
        }
    }

    #[cfg(target_os = "macos")]
    for candidate in bundle_candidates() {
        if candidate.is_dir() {
            return Some(AppLauncher::Bundle(candidate));
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("wtm-gui");
            if is_executable_file(&sibling) {
                return Some(AppLauncher::Binary(sibling));
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    for candidate in linux_binary_candidates() {
        if is_executable_file(&candidate) {
            return Some(AppLauncher::Binary(candidate));
        }
    }

    which("wtm-gui").map(AppLauncher::Binary)
}

/// Launch the app for `global`'s repository (or the current directory).
///
/// Returns `Ok(false)` when no app is installed, so the caller can fall back
/// without treating it as an error.
pub fn try_launch(global: &GlobalArgs) -> Result<bool> {
    let Some(launcher) = locate() else {
        return Ok(false);
    };

    // Resolve the repository here rather than in the app: the CLI knows the
    // working directory, and a Dock-launched app does not. A directory that
    // is not a repository is not fatal — the app opens on its sidebar.
    let repo = match crate::repo::discover(global.repo.as_deref()) {
        Ok(ctx) => Some(ctx.main_root),
        Err(_) => global.repo.clone(),
    };

    launcher.launch(repo.as_deref())?;
    Ok(true)
}

/// Standard locations for the application bundle. macOS only: see
/// [`AppLauncher::Bundle`].
#[cfg(target_os = "macos")]
fn bundle_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![PathBuf::from("/Applications/WTM.app")];
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join("Applications").join("WTM.app"));
    }
    candidates
}

/// Fixed locations a `wtm-gui` binary lands at on Linux, checked after the
/// sibling-of-`wtm` check and before the generic `$PATH` search: a per-user
/// install (`~/.local/bin`, the XDG convention for user-installed binaries),
/// then the two conventional system prefixes for one installed system-wide
/// (`/usr/local/bin` for a manual install, `/usr/bin` for a distro package).
#[cfg(not(target_os = "macos"))]
fn linux_binary_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".local/bin/wtm-gui"));
    }
    candidates.push(PathBuf::from("/usr/local/bin/wtm-gui"));
    candidates.push(PathBuf::from("/usr/bin/wtm-gui"));
    candidates
}

/// A `.app` directory is a bundle (macOS only -- see [`AppLauncher::Bundle`]);
/// anything else must be an executable file.
fn classify(path: &Path) -> Option<AppLauncher> {
    #[cfg(target_os = "macos")]
    if path.extension().is_some_and(|ext| ext == "app") && path.is_dir() {
        return Some(AppLauncher::Bundle(path.to_path_buf()));
    }
    if is_executable_file(path) {
        return Some(AppLauncher::Binary(path.to_path_buf()));
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// First executable named `name` on `$PATH`.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_file(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn classify_recognizes_a_bundle_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().join("WTM.app");
        std::fs::create_dir(&bundle).unwrap();

        assert_eq!(classify(&bundle), Some(AppLauncher::Bundle(bundle)));
    }

    #[test]
    fn classify_rejects_a_non_executable_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("wtm-gui");
        std::fs::write(&file, b"not executable").unwrap();

        assert_eq!(classify(&file), None);
    }

    #[cfg(unix)]
    #[test]
    fn classify_accepts_an_executable_binary() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("wtm-gui");
        std::fs::write(&file, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(classify(&file), Some(AppLauncher::Binary(file)));
    }
}

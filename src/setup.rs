//! Post-create automation: copy or symlink files from the main worktree into
//! a freshly created worktree, then run configured setup commands.

use std::fs;
use std::path::Path;
use std::process::Command;

use crate::config::{Config, CopyMode};
use crate::error::{Error, Result};

/// Run post-create automation in this order:
/// 1. For each setup.copy entry: source = main_root/path. Skip silently
///    (with a stderr note unless quiet) when the source does not exist. Never
///    overwrite an existing destination file. Create parent dirs. mode=copy
///    copies file contents+permissions; mode=symlink creates a symlink to the
///    ABSOLUTE source path.
/// 2. Run each setup.commands entry via `sh -c`, cwd = worktree, streaming
///    stdout/stderr to the user (inherit). First failing command ⇒
///    Error::Setup naming the command (the worktree stays).
pub fn run(config: &Config, main_root: &Path, worktree: &Path, quiet: bool) -> Result<()> {
    for entry in &config.setup.copy {
        copy_entry(main_root, worktree, &entry.path, entry.mode, quiet)?;
    }

    for command in &config.setup.commands {
        if !quiet {
            eprintln!("wtm: running setup command: {command}");
        }
        let status = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(worktree)
            .status()
            .map_err(|e| Error::Setup(format!("`{command}` could not be started: {e}")))?;
        if !status.success() {
            return Err(Error::Setup(format!("`{command}` failed ({status})")));
        }
    }

    Ok(())
}

/// Materialize one copy/symlink entry. Missing sources and already-existing
/// destinations are skipped (never an error, never an overwrite).
fn copy_entry(
    main_root: &Path,
    worktree: &Path,
    rel: &str,
    mode: CopyMode,
    quiet: bool,
) -> Result<()> {
    let source = main_root.join(rel);
    let dest = worktree.join(rel);

    if fs::symlink_metadata(&source).is_err() {
        if !quiet {
            eprintln!(
                "wtm: skipping '{rel}': not found in {}",
                main_root.display()
            );
        }
        return Ok(());
    }
    // symlink_metadata (not exists()) so even a broken symlink at the
    // destination blocks the write.
    if fs::symlink_metadata(&dest).is_ok() {
        if !quiet {
            eprintln!("wtm: skipping '{rel}': already exists in the new worktree");
        }
        return Ok(());
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    match mode {
        CopyMode::Symlink => {
            // Always link to the ABSOLUTE source path so the link survives
            // being viewed from any cwd.
            let absolute = fs::canonicalize(&source).unwrap_or(source);
            #[cfg(unix)]
            std::os::unix::fs::symlink(&absolute, &dest)?;
            #[cfg(not(unix))]
            {
                let _ = absolute;
                return Err(Error::Other(format!(
                    "symlink mode for '{rel}' is not supported on this platform"
                )));
            }
        }
        CopyMode::Copy => copy_recursive(&source, &dest)?,
    }
    Ok(())
}

/// Copy a file (contents + permissions, via `fs::copy`) or a directory tree.
fn copy_recursive(source: &Path, dest: &Path) -> Result<()> {
    let meta = fs::metadata(source)?;
    if meta.is_dir() {
        fs::create_dir_all(dest)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dest.join(entry.file_name()))?;
        }
        fs::set_permissions(dest, meta.permissions())?;
    } else {
        fs::copy(source, dest)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CopyEntry, SetupConfig};

    fn make_config(copy: Vec<CopyEntry>, commands: Vec<String>) -> Config {
        Config {
            setup: SetupConfig { commands, copy },
            ..Config::default()
        }
    }

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let main = tmp.path().join("main");
        let wt = tmp.path().join("wt");
        fs::create_dir(&main).unwrap();
        fs::create_dir(&wt).unwrap();
        (tmp, main, wt)
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn copies_file_with_contents_and_permissions() {
        let (_tmp, main, wt) = fixture();
        fs::write(main.join(".env"), "SECRET=1\n").unwrap();
        #[cfg(unix)]
        set_mode(&main.join(".env"), 0o600);

        let config = make_config(
            vec![CopyEntry {
                path: ".env".to_string(),
                mode: CopyMode::Copy,
            }],
            vec![],
        );
        run(&config, &main, &wt, true).unwrap();

        assert_eq!(fs::read_to_string(wt.join(".env")).unwrap(), "SECRET=1\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(wt.join(".env")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn creates_parent_directories_for_nested_entries() {
        let (_tmp, main, wt) = fixture();
        fs::create_dir_all(main.join("config").join("dev")).unwrap();
        fs::write(main.join("config").join("dev").join("local.toml"), "x").unwrap();

        let config = make_config(
            vec![CopyEntry {
                path: "config/dev/local.toml".to_string(),
                mode: CopyMode::Copy,
            }],
            vec![],
        );
        run(&config, &main, &wt, true).unwrap();
        assert!(wt.join("config").join("dev").join("local.toml").exists());
    }

    #[test]
    fn missing_source_is_skipped_without_error() {
        let (_tmp, main, wt) = fixture();
        let config = make_config(
            vec![CopyEntry {
                path: "does-not-exist.txt".to_string(),
                mode: CopyMode::Copy,
            }],
            vec![],
        );
        run(&config, &main, &wt, true).unwrap();
        assert!(!wt.join("does-not-exist.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_points_to_absolute_source() {
        let (_tmp, main, wt) = fixture();
        fs::write(main.join(".env"), "SECRET=1\n").unwrap();

        let config = make_config(
            vec![CopyEntry {
                path: ".env".to_string(),
                mode: CopyMode::Symlink,
            }],
            vec![],
        );
        run(&config, &main, &wt, true).unwrap();

        let link = wt.join(".env");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        let target = fs::read_link(&link).unwrap();
        assert!(target.is_absolute());
        assert_eq!(target, fs::canonicalize(main.join(".env")).unwrap());
        assert_eq!(fs::read_to_string(&link).unwrap(), "SECRET=1\n");
    }

    #[test]
    fn never_overwrites_existing_destination() {
        let (_tmp, main, wt) = fixture();
        fs::write(main.join(".env"), "FROM_MAIN\n").unwrap();
        fs::write(wt.join(".env"), "KEEP_ME\n").unwrap();

        let config = make_config(
            vec![CopyEntry {
                path: ".env".to_string(),
                mode: CopyMode::Copy,
            }],
            vec![],
        );
        run(&config, &main, &wt, true).unwrap();
        assert_eq!(fs::read_to_string(wt.join(".env")).unwrap(), "KEEP_ME\n");
    }

    #[test]
    fn commands_run_in_worktree_cwd() {
        let (_tmp, main, wt) = fixture();
        let config = make_config(vec![], vec!["printf created > marker.txt".to_string()]);
        run(&config, &main, &wt, true).unwrap();
        assert_eq!(
            fs::read_to_string(wt.join("marker.txt")).unwrap(),
            "created"
        );
    }

    #[test]
    fn first_failing_command_reports_setup_error() {
        let (_tmp, main, wt) = fixture();
        let config = make_config(
            vec![],
            vec![
                "true".to_string(),
                "exit 7".to_string(),
                "printf never > should-not-exist.txt".to_string(),
            ],
        );
        let err = run(&config, &main, &wt, true).unwrap_err();
        match err {
            Error::Setup(msg) => assert!(msg.contains("exit 7"), "{msg}"),
            other => panic!("expected Setup error, got {other}"),
        }
        assert!(
            !wt.join("should-not-exist.txt").exists(),
            "commands after the failure must not run"
        );
    }
}

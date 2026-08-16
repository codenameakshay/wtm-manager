//! Post-create automation: copy or symlink files from the main worktree into
//! a freshly created worktree, then run configured setup commands.
//!
//! Two entry points share the copy step (`copy_entry`) and differ only in
//! how command output reaches the user: [`run`] inherits the child's
//! stdout/stderr straight into the CLI's own (a terminal, or whatever the
//! caller redirected them to), while [`run_streaming`] captures both and
//! reports them line-by-line through a callback for callers with no stdio to
//! inherit into (the GUI). They are deliberately NOT unified into one
//! implementation: inheriting stdio keeps the CLI's stdout/stderr streams
//! separate (so e.g. `wtm add feat > out.txt` still puts the setup command's
//! stdout in `out.txt`), whereas streaming necessarily merges both into one
//! sequence of lines — folding that back through `run`'s stderr would
//! silently change where a setup command's stdout ends up for CLI users.

use std::fs;
#[cfg(not(unix))]
use std::fs::OpenOptions;
#[cfg(not(unix))]
use std::io;
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

use crate::config::{Config, CopyMode};
use crate::error::{Error, Result};

/// Run post-create automation in this order:
/// 1. For each setup.copy entry, validate that the relative source remains
///    inside the main worktree and that neither source nor destination walks
///    through symlinks. Missing sources and existing destinations are skipped.
///    Copy mode preserves permissions; symlink mode targets the contained
///    absolute source.
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

/// One step of post-create setup, reported as it happens by
/// [`run_streaming`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupEvent {
    CopyStarted {
        path: String,
    },
    CopyFinished {
        path: String,
    },
    CommandStarted {
        command: String,
    },
    /// One line of combined stdout/stderr from the running command, in
    /// arrival order. Interleaving between the two streams is best-effort
    /// (they are read on separate threads); ordering within a single stream
    /// is exact.
    CommandOutput {
        line: String,
    },
    CommandFinished {
        command: String,
        success: bool,
    },
}

/// Same steps and semantics as [`run`] — same ordering (copies before
/// commands), same error type/messages, same behavior on failure — but the
/// child's stdout and stderr are captured and reported through `sink`
/// instead of inherited, for callers with no terminal to inherit into.
///
/// Copy entries always run quiet (there is nothing useful to `eprintln!` to);
/// `sink` still gets a `CopyStarted`/`CopyFinished` pair around each one so a
/// caller can show progress.
pub fn run_streaming(
    config: &Config,
    main_root: &Path,
    worktree: &Path,
    sink: &mut dyn FnMut(SetupEvent),
) -> Result<()> {
    for entry in &config.setup.copy {
        sink(SetupEvent::CopyStarted {
            path: entry.path.clone(),
        });
        copy_entry(main_root, worktree, &entry.path, entry.mode, true)?;
        sink(SetupEvent::CopyFinished {
            path: entry.path.clone(),
        });
    }

    for command in &config.setup.commands {
        sink(SetupEvent::CommandStarted {
            command: command.clone(),
        });
        run_command_streaming(command, worktree, sink)?;
    }

    Ok(())
}

/// Spawn `command` via `sh -c`, cwd = `worktree`, with stdout and stderr
/// piped and each read on its own thread so a long-running command (e.g. a
/// build) streams output as it happens instead of only at exit. Both threads
/// forward lines into one channel; the calling thread drains it into `sink`
/// as `CommandOutput` events. The channel closes itself once both reader
/// threads finish (each owns its own `Sender` clone), so the `for line in
/// rx` loop ends exactly when there is no more output to deliver.
fn run_command_streaming(
    command: &str,
    worktree: &Path,
    sink: &mut dyn FnMut(SetupEvent),
) -> Result<()> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(worktree)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Setup(format!("`{command}` could not be started: {e}")))?;

    let stdout = child.stdout.take().expect("stdout was piped at spawn");
    let stderr = child.stderr.take().expect("stderr was piped at spawn");
    let (tx, rx) = mpsc::channel::<String>();
    let tx_stderr = tx.clone();
    let stdout_reader = thread::spawn(move || forward_lines(stdout, tx));
    let stderr_reader = thread::spawn(move || forward_lines(stderr, tx_stderr));

    for line in rx {
        sink(SetupEvent::CommandOutput { line });
    }
    // Both threads have already dropped their `Sender` (the channel just
    // closed), so they are done or finishing; this join is not a stall.
    let _ = stdout_reader.join();
    let _ = stderr_reader.join();

    let status = child
        .wait()
        .map_err(|e| Error::Setup(format!("`{command}` could not be started: {e}")))?;
    sink(SetupEvent::CommandFinished {
        command: command.to_string(),
        success: status.success(),
    });
    if !status.success() {
        return Err(Error::Setup(format!("`{command}` failed ({status})")));
    }
    Ok(())
}

/// Read `reader` line by line (splitting on `\n`, trimming a trailing `\r`),
/// sending each line to `tx`. Invalid UTF-8 is replaced rather than failing
/// the read: setup command output is diagnostic text for a human, not a
/// payload that must round-trip exactly.
fn forward_lines(reader: impl Read, tx: mpsc::Sender<String>) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {
                if buf.last() == Some(&b'\n') {
                    buf.pop();
                    if buf.last() == Some(&b'\r') {
                        buf.pop();
                    }
                }
                if tx.send(String::from_utf8_lossy(&buf).into_owned()).is_err() {
                    break; // Receiver went away; nothing left to do.
                }
            }
            Err(_) => break,
        }
    }
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
    let rel_path = validate_relative_path(rel)?;
    let main_root = fs::canonicalize(main_root)?;
    let worktree = fs::canonicalize(worktree)?;
    let source = main_root.join(rel_path);

    match fs::symlink_metadata(&source) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !quiet {
                eprintln!(
                    "wtm: skipping '{rel}': not found in {}",
                    main_root.display()
                );
            }
            return Ok(());
        }
        Err(e) => return Err(Error::Io(e)),
    }

    let canonical_source = fs::canonicalize(&source).map_err(|e| {
        Error::Setup(format!(
            "setup.copy path '{rel}' cannot be resolved safely: {e}"
        ))
    })?;
    if !canonical_source.starts_with(&main_root) {
        return Err(Error::Setup(format!(
            "setup.copy path '{rel}' escapes the main worktree"
        )));
    }

    #[cfg(unix)]
    let created = materialize_at(&main_root, &worktree, rel_path, rel, mode)?;
    #[cfg(not(unix))]
    let created = materialize_path_based(&main_root, &worktree, rel_path, rel, mode)?;
    if !created && !quiet {
        eprintln!("wtm: skipping '{rel}': already exists in the new worktree");
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<&Path> {
    let path = Path::new(path);
    let mut has_normal_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_normal_component = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::Setup(format!(
                    "setup.copy path '{}' must be a contained relative path without '..'",
                    path.display()
                )));
            }
        }
    }
    if !has_normal_component {
        return Err(Error::Setup(
            "setup.copy path must be a non-empty relative path".to_string(),
        ));
    }
    Ok(path)
}

#[cfg(not(unix))]
fn materialize_path_based(
    main_root: &Path,
    worktree: &Path,
    rel: &Path,
    display: &str,
    mode: CopyMode,
) -> Result<bool> {
    let source = main_root.join(rel);
    let dest = worktree.join(rel);
    ensure_destination_contained(worktree, rel, display)?;
    if fs::symlink_metadata(&dest).is_ok() {
        return Ok(false);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
        ensure_destination_contained(worktree, rel, display)?;
    }
    match mode {
        CopyMode::Copy => copy_recursive(&source, &dest)?,
        CopyMode::Symlink => {
            return Err(Error::Other(format!(
                "symlink mode for '{display}' is not supported on this platform"
            )))
        }
    }
    Ok(true)
}

#[cfg(not(unix))]
fn ensure_destination_contained(worktree: &Path, rel: &Path, display: &str) -> Result<()> {
    let mut current = worktree.to_path_buf();
    for component in rel.components() {
        if let Component::Normal(component) = component {
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(Error::Setup(format!(
                        "setup.copy path '{display}' escapes the new worktree through a symlink"
                    )));
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(e) => return Err(Error::Io(e)),
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn copy_recursive(source: &Path, dest: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() {
        return Err(Error::Setup(format!(
            "setup.copy source '{}' contains a symlink",
            source.display()
        )));
    }
    if meta.is_dir() {
        fs::create_dir(dest)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dest.join(entry.file_name()))?;
        }
        fs::set_permissions(dest, meta.permissions())?;
    } else if meta.is_file() {
        copy_file_no_follow(source, dest, meta.permissions())?;
    } else {
        return Err(Error::Setup(format!(
            "setup.copy source '{}' is not a regular file or directory",
            source.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn copy_file_no_follow(source: &Path, dest: &Path, permissions: fs::Permissions) -> Result<()> {
    let mut source_options = OpenOptions::new();
    source_options.read(true);
    let mut dest_options = OpenOptions::new();
    dest_options.write(true).create_new(true);
    let mut input = source_options.open(source)?;
    if !input.metadata()?.is_file() {
        return Err(Error::Setup(format!(
            "setup.copy source '{}' is not a regular file",
            source.display()
        )));
    }
    let mut output = dest_options.open(dest)?;
    io::copy(&mut input, &mut output)?;
    output.set_permissions(permissions)?;
    Ok(())
}

#[cfg(unix)]
fn materialize_at(
    main_root: &Path,
    worktree: &Path,
    rel: &Path,
    display: &str,
    mode: CopyMode,
) -> Result<bool> {
    use std::ffi::OsStr;

    let components: Vec<&OsStr> = rel
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name),
            Component::CurDir => None,
            _ => unreachable!("validated relative path"),
        })
        .collect();
    let (name, parents) = components.split_last().expect("validated non-empty path");
    let mut parent = open_dir_path(worktree)?;
    for component in parents {
        parent = open_or_create_dir_at(&parent, component, display)?;
    }
    if entry_exists_at(&parent, name)? {
        return Ok(false);
    }

    let source = main_root.join(rel);
    match mode {
        CopyMode::Copy => copy_recursive_at(&source, &parent, name)?,
        CopyMode::Symlink => {
            let meta = fs::symlink_metadata(&source)?;
            let target = fs::canonicalize(&source)?;
            if meta.file_type().is_symlink() || !meta.is_file() || !target.starts_with(main_root) {
                return Err(Error::Setup(format!(
                    "setup.copy symlink source '{display}' must be a contained regular file"
                )));
            }
            symlink_at(&target, &parent, name)?;
        }
    }
    Ok(true)
}

#[cfg(unix)]
fn copy_recursive_at(
    source: &Path,
    dest_parent: &std::os::fd::OwnedFd,
    dest_name: &std::ffi::OsStr,
) -> Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;

    let meta = fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() {
        return Err(Error::Setup(format!(
            "setup.copy source '{}' contains a symlink",
            source.display()
        )));
    }
    if meta.is_dir() {
        mkdir_at(dest_parent, dest_name, 0o700)?;
        let dest = open_dir_at(dest_parent, dest_name).map_err(|error| {
            Error::Setup(format!(
                "setup.copy destination '{}' could not be opened safely: {error}",
                source.display()
            ))
        })?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_recursive_at(&entry.path(), &dest, &entry.file_name())?;
        }
        let mode = meta.permissions().mode() & 0o7777;
        if unsafe { libc::fchmod(dest.as_raw_fd(), mode as libc::mode_t) } != 0 {
            return Err(Error::Io(std::io::Error::last_os_error()));
        }
    } else if meta.is_file() {
        copy_file_at(source, dest_parent, dest_name, meta.permissions())?;
    } else {
        return Err(Error::Setup(format!(
            "setup.copy source '{}' is not a regular file or directory",
            source.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn copy_file_at(
    source: &Path,
    dest_parent: &std::os::fd::OwnedFd,
    dest_name: &std::ffi::OsStr,
    permissions: fs::Permissions,
) -> Result<()> {
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut source_options = fs::OpenOptions::new();
    source_options.read(true).custom_flags(libc::O_NOFOLLOW);
    let mut input = source_options.open(source)?;
    if !input.metadata()?.is_file() {
        return Err(Error::Setup(format!(
            "setup.copy source '{}' is not a regular file",
            source.display()
        )));
    }

    let name = path_c_string(dest_name)?;
    let raw = unsafe {
        libc::openat(
            dest_parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if raw < 0 {
        return Err(Error::Io(std::io::Error::last_os_error()));
    }
    let owned = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut output = fs::File::from(owned);
    io::copy(&mut input, &mut output)?;
    let mode = permissions.mode() & 0o7777;
    if unsafe { libc::fchmod(output.as_raw_fd(), mode as libc::mode_t) } != 0 {
        return Err(Error::Io(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(unix)]
fn open_dir_path(path: &Path) -> Result<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;

    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::Setup("setup.copy path contains a NUL byte".to_string()))?;
    let raw = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(Error::Io(std::io::Error::last_os_error()));
    }
    Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) })
}

#[cfg(unix)]
fn open_or_create_dir_at(
    parent: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
    display: &str,
) -> Result<std::os::fd::OwnedFd> {
    match open_dir_at(parent, name) {
        Ok(directory) => Ok(directory),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            mkdir_at(parent, name, 0o777)?;
            open_dir_at(parent, name).map_err(Error::Io)
        }
        Err(error) => Err(Error::Setup(format!(
            "setup.copy path '{display}' escapes the new worktree through a symlink or non-directory parent: {error}"
        ))),
    }
}

#[cfg(unix)]
fn open_dir_at(
    parent: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let name = path_c_string_io(name)?;
    let raw = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) })
}

#[cfg(unix)]
fn mkdir_at(
    parent: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
    mode: libc::mode_t,
) -> Result<()> {
    use std::os::fd::AsRawFd;

    let name = path_c_string(name)?;
    if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), mode) } != 0 {
        return Err(Error::Io(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(unix)]
fn entry_exists_at(parent: &std::os::fd::OwnedFd, name: &std::ffi::OsStr) -> Result<bool> {
    use std::os::fd::AsRawFd;

    let name = path_c_string(name)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        Ok(true)
    } else {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            Ok(false)
        } else {
            Err(Error::Io(error))
        }
    }
}

#[cfg(unix)]
fn symlink_at(target: &Path, parent: &std::os::fd::OwnedFd, name: &std::ffi::OsStr) -> Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    let target = std::ffi::CString::new(target.as_os_str().as_bytes())
        .map_err(|_| Error::Setup("setup.copy target contains a NUL byte".to_string()))?;
    let name = path_c_string(name)?;
    if unsafe { libc::symlinkat(target.as_ptr(), parent.as_raw_fd(), name.as_ptr()) } != 0 {
        return Err(Error::Io(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(unix)]
fn path_c_string(name: &std::ffi::OsStr) -> Result<std::ffi::CString> {
    path_c_string_io(name).map_err(Error::Io)
}

#[cfg(unix)]
fn path_c_string_io(name: &std::ffi::OsStr) -> std::io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;

    std::ffi::CString::new(name.as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "path contains NUL"))
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

    fn assert_setup_error(err: Error, expected: &str) {
        match err {
            Error::Setup(message) => assert!(message.contains(expected), "{message}"),
            other => panic!("expected setup error, got {other:?}"),
        }
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    fn symlink(path: &Path, target: &Path) {
        std::os::unix::fs::symlink(target, path).unwrap();
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

    #[test]
    fn rejects_absolute_copy_paths() {
        let (_tmp, main, wt) = fixture();
        let config = make_config(
            vec![CopyEntry {
                path: "/tmp/outside".to_string(),
                mode: CopyMode::Copy,
            }],
            vec![],
        );

        let err = run(&config, &main, &wt, true).unwrap_err();
        assert_setup_error(err, "relative path");
    }

    #[test]
    fn rejects_parent_traversal_copy_paths() {
        let (_tmp, main, wt) = fixture();
        let config = make_config(
            vec![CopyEntry {
                path: "../outside".to_string(),
                mode: CopyMode::Copy,
            }],
            vec![],
        );

        let err = run(&config, &main, &wt, true).unwrap_err();
        assert_setup_error(err, "relative path");
    }

    #[test]
    fn rejects_empty_copy_paths() {
        let (_tmp, main, wt) = fixture();
        let config = make_config(
            vec![CopyEntry {
                path: String::new(),
                mode: CopyMode::Copy,
            }],
            vec![],
        );

        let err = run(&config, &main, &wt, true).unwrap_err();
        assert_setup_error(err, "relative path");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_copy_sources_that_resolve_outside_main_root() {
        let (_tmp, main, wt) = fixture();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.env"), "SECRET=1\n").unwrap();
        symlink(&main.join("escape"), outside.path());

        let config = make_config(
            vec![CopyEntry {
                path: "escape/secret.env".to_string(),
                mode: CopyMode::Copy,
            }],
            vec![],
        );

        let err = run(&config, &main, &wt, true).unwrap_err();
        assert_setup_error(err, "escapes the main worktree");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_destination_parent_symlinks() {
        let (_tmp, main, wt) = fixture();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(main.join("nested")).unwrap();
        fs::write(main.join("nested").join("config.toml"), "key = 1\n").unwrap();
        symlink(&wt.join("nested"), outside.path());

        let config = make_config(
            vec![CopyEntry {
                path: "nested/config.toml".to_string(),
                mode: CopyMode::Copy,
            }],
            vec![],
        );

        let err = run(&config, &main, &wt, true).unwrap_err();
        assert_setup_error(err, "escapes the new worktree");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_children_during_recursive_copy() {
        let (_tmp, main, wt) = fixture();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(main.join("config")).unwrap();
        fs::write(main.join("config").join("local.toml"), "a = 1\n").unwrap();
        symlink(&main.join("config").join("linked"), outside.path());

        let config = make_config(
            vec![CopyEntry {
                path: "config".to_string(),
                mode: CopyMode::Copy,
            }],
            vec![],
        );

        let err = run(&config, &main, &wt, true).unwrap_err();
        assert_setup_error(err, "contains a symlink");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_cycles_during_recursive_copy() {
        let (_tmp, main, wt) = fixture();
        fs::create_dir(main.join("config")).unwrap();
        symlink(&main.join("config").join("loop"), &main.join("config"));

        let config = make_config(
            vec![CopyEntry {
                path: "config".to_string(),
                mode: CopyMode::Copy,
            }],
            vec![],
        );

        let err = run(&config, &main, &wt, true).unwrap_err();
        assert_setup_error(err, "contains a symlink");
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

    #[cfg(unix)]
    #[test]
    fn symlink_mode_rejects_directory_sources() {
        let (_tmp, main, wt) = fixture();
        fs::create_dir(main.join("config")).unwrap();

        let config = make_config(
            vec![CopyEntry {
                path: "config".to_string(),
                mode: CopyMode::Symlink,
            }],
            vec![],
        );

        let err = run(&config, &main, &wt, true).unwrap_err();
        assert_setup_error(err, "regular file");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_mode_rejects_sources_outside_main_root() {
        let (_tmp, main, wt) = fixture();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.env"), "SECRET=1\n").unwrap();
        symlink(
            &main.join("external.env"),
            &outside.path().join("secret.env"),
        );

        let config = make_config(
            vec![CopyEntry {
                path: "external.env".to_string(),
                mode: CopyMode::Symlink,
            }],
            vec![],
        );

        let err = run(&config, &main, &wt, true).unwrap_err();
        assert_setup_error(err, "escapes the main worktree");
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

    #[test]
    fn run_streaming_reports_copies_before_commands_and_command_output_lines() {
        let (_tmp, main, wt) = fixture();
        fs::write(main.join(".env"), "SECRET=1\n").unwrap();
        let config = make_config(
            vec![CopyEntry {
                path: ".env".to_string(),
                mode: CopyMode::Copy,
            }],
            vec!["printf 'line1\\nline2\\n'".to_string()],
        );

        let mut events = Vec::new();
        run_streaming(&config, &main, &wt, &mut |event| events.push(event)).unwrap();

        assert_eq!(fs::read_to_string(wt.join(".env")).unwrap(), "SECRET=1\n");
        assert_eq!(
            events,
            vec![
                SetupEvent::CopyStarted {
                    path: ".env".to_string()
                },
                SetupEvent::CopyFinished {
                    path: ".env".to_string()
                },
                SetupEvent::CommandStarted {
                    command: "printf 'line1\\nline2\\n'".to_string()
                },
                SetupEvent::CommandOutput {
                    line: "line1".to_string()
                },
                SetupEvent::CommandOutput {
                    line: "line2".to_string()
                },
                SetupEvent::CommandFinished {
                    command: "printf 'line1\\nline2\\n'".to_string(),
                    success: true,
                },
            ]
        );
    }

    #[test]
    fn run_streaming_captures_output_from_both_stdout_and_stderr() {
        let (_tmp, main, wt) = fixture();
        let config = make_config(
            vec![],
            vec!["printf 'out\\n'; printf 'err\\n' >&2".to_string()],
        );

        let mut events = Vec::new();
        run_streaming(&config, &main, &wt, &mut |event| events.push(event)).unwrap();

        let mut lines: Vec<String> = events
            .iter()
            .filter_map(|event| match event {
                SetupEvent::CommandOutput { line } => Some(line.clone()),
                _ => None,
            })
            .collect();
        lines.sort();
        assert_eq!(lines, vec!["err".to_string(), "out".to_string()]);
    }

    #[test]
    fn run_streaming_reports_failure_and_stops_before_running_next_command() {
        let (_tmp, main, wt) = fixture();
        let config = make_config(
            vec![],
            vec![
                "printf started".to_string(),
                "exit 7".to_string(),
                "printf never > should-not-exist.txt".to_string(),
            ],
        );

        let mut events = Vec::new();
        let err = run_streaming(&config, &main, &wt, &mut |event| events.push(event)).unwrap_err();
        match err {
            Error::Setup(msg) => assert!(msg.contains("exit 7"), "{msg}"),
            other => panic!("expected Setup error, got {other}"),
        }
        assert!(
            !wt.join("should-not-exist.txt").exists(),
            "commands after the failure must not run"
        );
        assert_eq!(
            events,
            vec![
                SetupEvent::CommandStarted {
                    command: "printf started".to_string()
                },
                SetupEvent::CommandOutput {
                    line: "started".to_string()
                },
                SetupEvent::CommandFinished {
                    command: "printf started".to_string(),
                    success: true,
                },
                SetupEvent::CommandStarted {
                    command: "exit 7".to_string()
                },
                SetupEvent::CommandFinished {
                    command: "exit 7".to_string(),
                    success: false,
                },
            ]
        );
    }
}

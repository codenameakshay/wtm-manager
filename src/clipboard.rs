//! System clipboard writer shared by the TUI and the GUI.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::error::Error;

/// Copy `text` to the system clipboard via the first available platform
/// tool (`pbcopy` on macOS; elsewhere `wl-copy`, `xclip -selection
/// clipboard`, then `xsel -ib`), returning that tool's name.
pub fn copy(text: &str) -> crate::error::Result<&'static str> {
    #[cfg(target_os = "macos")]
    let tools: &[(&str, &[&str])] = &[("pbcopy", &[])];
    #[cfg(not(target_os = "macos"))]
    let tools: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["-ib"]),
    ];

    for (tool, args) in tools {
        let Ok(mut child) = Command::new(tool)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue; // Not installed; try the next tool.
        };

        let write_ok = child
            .stdin
            .as_mut()
            .is_some_and(|stdin| stdin.write_all(text.as_bytes()).is_ok());
        // Closing stdin is what tells wl-copy/xclip/xsel to stop reading and
        // fork into the background as the selection's owner, so it must
        // happen even after a failed write; a partial write kills the child
        // first so a half-written value never becomes the clipboard.
        drop(child.stdin.take());
        if !write_ok {
            let _ = child.kill();
            let _ = child.wait();
            continue;
        }
        match child.wait() {
            Ok(status) if status.success() => return Ok(*tool),
            _ => continue,
        }
    }

    Err(Error::Other(format!(
        "no clipboard tool available (tried: {})",
        tools
            .iter()
            .map(|(tool, _)| *tool)
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

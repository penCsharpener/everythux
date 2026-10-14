//! Putting files on the clipboard so a file manager can paste them.
//!
//! egui only offers plain text, and file managers paste files from the `text/uri-list` target.
//! Setting a clipboard target needs a helper: `wl-copy` (wl-clipboard) on Wayland, `xclip` on X11.

use std::ffi::OsStr;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::error::PlatformError;
use crate::uri::file_uri;

const MIME: &str = "text/uri-list";

/// The `text/uri-list` body for `paths`: one `file://` URI per line, CRLF separated (RFC 2483).
#[must_use]
pub fn uri_list(paths: &[PathBuf]) -> String {
    let mut out = String::new();
    for path in paths {
        out.push_str(&file_uri(path));
        out.push_str("\r\n");
    }
    out
}

/// Which helper program was used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// `wl-copy`.
    WlCopy,
    /// `xclip`.
    Xclip,
}

/// Copies `paths` to the clipboard as a file list (`text/uri-list`).
///
/// Blocking: call from a worker thread.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] when no helper is installed for the session type;
/// [`PlatformError::Spawn`] when it fails to start.
pub fn copy_files(paths: &[PathBuf]) -> Result<Tool, PlatformError> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    let candidates: &[(Tool, &str, &[&str])] = if wayland {
        &[
            (Tool::WlCopy, "wl-copy", &["--type", MIME]),
            (
                Tool::Xclip,
                "xclip",
                &["-selection", "clipboard", "-t", MIME, "-i"],
            ),
        ]
    } else {
        &[
            (
                Tool::Xclip,
                "xclip",
                &["-selection", "clipboard", "-t", MIME, "-i"],
            ),
            (Tool::WlCopy, "wl-copy", &["--type", MIME]),
        ]
    };
    let body = uri_list(paths);
    for &(tool, program, args) in candidates {
        match run(program, args, &body) {
            Ok(()) => return Ok(tool),
            Err(PlatformError::Spawn { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(other) => return Err(other),
        }
    }
    Err(PlatformError::Unsupported(
        "copying files needs wl-clipboard (Wayland) or xclip (X11)",
    ))
}

fn run(program: &str, args: &[&str], body: &str) -> Result<(), PlatformError> {
    let spawn_error = |source| PlatformError::Spawn {
        program: program.to_owned(),
        source,
    };
    let mut child = Command::new(OsStr::new(program))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(spawn_error)?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(body.as_bytes()).map_err(spawn_error)?;
    }
    // Both tools fork into the background to keep serving the selection; the parent returns.
    let status = child.wait().map_err(spawn_error)?;
    if status.success() {
        Ok(())
    } else {
        Err(spawn_error(std::io::Error::other(format!(
            "{program} exited with {status}"
        ))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_list_is_crlf_separated_and_encoded() {
        let list = uri_list(&[PathBuf::from("/a b/c"), PathBuf::from("/d")]);
        assert_eq!(list, "file:///a%20b/c\r\nfile:///d\r\n");
        assert_eq!(uri_list(&[]), "");
    }
}

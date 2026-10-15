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
/// GTK file managers (Nautilus, Nemo, Thunar, …) read "copy"/"cut" plus the URIs from this target.
const GNOME_MIME: &str = "x-special/gnome-copied-files";

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
    set_clipboard(MIME, &uri_list(paths))
}

/// Puts `paths` on the clipboard as a *cut* file list (`x-special/gnome-copied-files`), so a
/// paste moves them. Same requirements and errors as [`copy_files`].
pub fn cut_files(paths: &[PathBuf]) -> Result<Tool, PlatformError> {
    set_clipboard(GNOME_MIME, &gnome_list(paths, true))
}

/// The `x-special/gnome-copied-files` body: the action, then one URI per line (no trailing newline).
#[must_use]
pub fn gnome_list(paths: &[PathBuf], cut: bool) -> String {
    let mut out = String::from(if cut { "cut" } else { "copy" });
    for path in paths {
        out.push('\n');
        out.push_str(&file_uri(path));
    }
    out
}

/// Parses a clipboard file list in either format: `x-special/gnome-copied-files` (first line
/// `cut` or `copy`) or `text/uri-list`. Returns the paths and whether they were cut. Anything that
/// is not a `file://` URI is ignored.
#[must_use]
pub fn parse_file_list(text: &str) -> (Vec<PathBuf>, bool) {
    use std::os::unix::ffi::OsStringExt as _;

    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    let mut cut = false;
    let mut first = lines.next();
    if let Some(action @ ("cut" | "copy")) = first {
        cut = action == "cut";
        first = lines.next();
    }
    let paths = first
        .into_iter()
        .chain(lines)
        .filter_map(|line| {
            let rest = line.strip_prefix("file://")?;
            // `file://localhost/path` and `file:///path` are both local.
            let rest = rest.strip_prefix("localhost").unwrap_or(rest);
            rest.starts_with('/').then(|| {
                let bytes: Vec<u8> = percent_encoding::percent_decode_str(rest).collect();
                PathBuf::from(std::ffi::OsString::from_vec(bytes))
            })
        })
        .collect();
    (paths, cut)
}

/// Reads the files on the clipboard and whether they were cut. Empty when it holds no file list.
///
/// Blocking: call from a worker thread.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] when no helper is installed for the session type.
pub fn read_files() -> Result<(Vec<PathBuf>, bool), PlatformError> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    let wl: fn(&str) -> (&'static str, Vec<String>) = |mime| {
        (
            "wl-paste",
            vec!["--no-newline".into(), "--type".into(), mime.into()],
        )
    };
    let xc: fn(&str) -> (&'static str, Vec<String>) = |mime| {
        (
            "xclip",
            vec![
                "-selection".into(),
                "clipboard".into(),
                "-o".into(),
                "-t".into(),
                mime.into(),
            ],
        )
    };
    let tools = if wayland { [wl, xc] } else { [xc, wl] };
    let mut found_tool = false;
    for mime in [GNOME_MIME, MIME] {
        for tool in tools {
            let (program, args) = tool(mime);
            match Command::new(program)
                .args(&args)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
            {
                Ok(out) => {
                    found_tool = true;
                    if out.status.success() {
                        let (paths, cut) = parse_file_list(&String::from_utf8_lossy(&out.stdout));
                        if !paths.is_empty() {
                            return Ok((paths, cut));
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(PlatformError::Spawn {
                        program: program.to_owned(),
                        source,
                    });
                }
            }
        }
    }
    if found_tool {
        Ok((Vec::new(), false))
    } else {
        Err(PlatformError::Unsupported(
            "pasting files needs wl-clipboard (Wayland) or xclip (X11)",
        ))
    }
}

fn set_clipboard(mime: &str, body: &str) -> Result<Tool, PlatformError> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    let candidates: &[(Tool, &str)] = if wayland {
        &[(Tool::WlCopy, "wl-copy"), (Tool::Xclip, "xclip")]
    } else {
        &[(Tool::Xclip, "xclip"), (Tool::WlCopy, "wl-copy")]
    };
    for &(tool, program) in candidates {
        let args: Vec<&str> = match tool {
            Tool::WlCopy => vec!["--type", mime],
            Tool::Xclip => vec!["-selection", "clipboard", "-t", mime, "-i"],
        };
        match run(program, &args, body) {
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

    #[test]
    fn file_lists_round_trip_in_both_formats() {
        let paths = vec![PathBuf::from("/a b/c"), PathBuf::from("/d#1")];
        assert_eq!(parse_file_list(&gnome_list(&paths, true)), (paths.clone(), true));
        assert_eq!(
            parse_file_list(&gnome_list(&paths, false)),
            (paths.clone(), false)
        );
        assert_eq!(parse_file_list(&uri_list(&paths)), (paths, false));
        assert_eq!(parse_file_list("https://x/y\n# c\n"), (Vec::new(), false));
    }
}

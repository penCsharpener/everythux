//! File operations started from the UI (ROADMAP-v0.5 Phase D). Blocking: call from a worker thread.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Why a rename failed. Each variant has a user-facing message (`Display`).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RenameError {
    /// The new name is empty, `.`/`..`, contains `/` or NUL, or is longer than 255 bytes.
    #[error("invalid name: {0}")]
    InvalidName(&'static str),
    /// Another file or folder already has the new name (nothing was overwritten).
    #[error("a file or folder with that name already exists")]
    AlreadyExists,
    /// No permission to rename in this folder.
    #[error("permission denied")]
    PermissionDenied,
    /// The file no longer exists.
    #[error("the file no longer exists")]
    NotFound,
    /// The item is a mount point or otherwise busy.
    #[error("the item is in use (for example a mount point)")]
    Busy,
    /// The filesystem is mounted read-only.
    #[error("the filesystem is read-only")]
    ReadOnlyFilesystem,
    /// Anything else.
    #[error(transparent)]
    Other(std::io::Error),
}

/// Validates `new_name` as a single path component (no filesystem access).
pub fn validate_name(new_name: &OsStr) -> Result<(), RenameError> {
    let _ = new_name;
    todo!("Phase D")
}

/// Renames `path` to `new_name` inside the same folder, atomically and **never overwriting** an
/// existing entry (`renameat2(RENAME_NOREPLACE)`). Returns the new absolute path.
pub fn rename_in_place(path: &Path, new_name: &OsStr) -> Result<PathBuf, RenameError> {
    let _ = (path, new_name);
    todo!("Phase D")
}

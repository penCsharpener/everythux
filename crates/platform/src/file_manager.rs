//! File manager integration (ROADMAP Phase 5).

use std::path::Path;

use crate::PlatformError;

/// How a request was fulfilled (for logging / status bar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// `org.freedesktop.FileManager1` on the session bus.
    FileManager1,
    /// `org.freedesktop.portal.OpenURI`.
    Portal,
    /// `xdg-open` (no selection / properties possible).
    XdgOpen,
}

/// Opens the folder containing `path` with `path` selected (Path column double-click).
pub fn show_item(path: &Path) -> Result<Method, PlatformError> {
    let _ = path;
    todo!("Phase 5")
}

/// Opens the file manager's properties dialog for `path` (Size/Modified double-click).
/// Returns `Err(PlatformError::Unsupported)` if no file manager implements
/// `ShowItemProperties`; the UI then shows its built-in dialog.
pub fn show_properties(path: &Path) -> Result<Method, PlatformError> {
    let _ = path;
    todo!("Phase 5")
}

/// Opens `path` with its default application (Name column double-click / Enter).
pub fn open_default(path: &Path) -> Result<Method, PlatformError> {
    let _ = path;
    todo!("Phase 5")
}

//! UDisks2 over the system bus (ROADMAP Phase 7 "Volumes panel").

use std::path::PathBuf;

use crate::PlatformError;

/// A block device carrying a filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockDevice {
    /// D-Bus object path (needed to mount).
    pub object_path: String,
    /// Device node, e.g. `/dev/sdb1`.
    pub device: PathBuf,
    /// Filesystem UUID.
    pub fs_uuid: Option<String>,
    /// Filesystem type, e.g. `ext4`.
    pub fs_type: String,
    /// Label.
    pub label: Option<String>,
    /// Size in bytes.
    pub size: u64,
    /// Current mount points (empty = unmounted).
    pub mount_points: Vec<PathBuf>,
}

/// Lists filesystems of the given types known to UDisks2. `Err` if UDisks2 isn't available
/// (the UI then shows only mounted volumes from mountinfo).
pub fn list_filesystems(fs_types: &[String]) -> Result<Vec<BlockDevice>, PlatformError> {
    let _ = fs_types;
    todo!()
}

/// Mounts a filesystem via UDisks2 (polkit may prompt) and returns the mount point.
pub fn mount(object_path: &str) -> Result<PathBuf, PlatformError> {
    let _ = object_path;
    todo!()
}

//! Finds the volumes to index by parsing `/proc/self/mountinfo` (ROADMAP Phase 1).

use std::path::PathBuf;

use everythux_core::config::VolumesConfig;

use crate::IndexError;

/// A mounted filesystem selected for indexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredVolume {
    /// Filesystem UUID (`/dev/disk/by-uuid`), or `dev:<major>:<minor>` if none.
    pub fs_uuid: String,
    /// Block device (mount source), e.g. `/dev/nvme0n1p2`.
    pub device: PathBuf,
    /// Shortest mount point of this filesystem root (bind mounts are deduplicated).
    pub mount_point: PathBuf,
    /// `st_dev` of the mount point, used to stay on one filesystem.
    pub dev: u64,
    /// Filesystem type, e.g. `ext4`.
    pub fs_type: String,
    /// Label from `/dev/disk/by-label`, if any.
    pub label: Option<String>,
}

/// Discovers mounted volumes whose type is in `config.auto_discover_fs_types`, plus
/// `config.include` roots, minus `config.disabled_uuids`.
pub fn discover_volumes(config: &VolumesConfig) -> Result<Vec<DiscoveredVolume>, IndexError> {
    let _ = config;
    todo!("Phase 1")
}

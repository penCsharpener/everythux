//! Plain data types describing volumes and entries.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What kind of filesystem object an entry is. Stored as `u8` in SQLite and in the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum EntryKind {
    /// Regular file.
    File = 0,
    /// Directory.
    Dir = 1,
    /// Symbolic link (never followed by default).
    Symlink = 2,
    /// Sockets, FIFOs, device nodes.
    Other = 3,
}

impl EntryKind {
    /// Decodes the `u8` representation; unknown values map to [`EntryKind::Other`].
    pub const fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::File,
            1 => Self::Dir,
            2 => Self::Symlink,
            _ => Self::Other,
        }
    }

    /// Whether this entry is a directory.
    pub const fn is_dir(self) -> bool {
        matches!(self, Self::Dir)
    }
}

/// Lifecycle state of an indexed volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeState {
    /// Mounted and indexed.
    Online,
    /// Known (entries kept in the index) but currently not mounted.
    Offline,
    /// A scan is running.
    Scanning,
    /// The last scan failed.
    Error,
}

impl VolumeState {
    /// The string stored in SQLite.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Offline => "offline",
            Self::Scanning => "scanning",
            Self::Error => "error",
        }
    }

    /// Parses the SQLite representation; unknown values map to [`VolumeState::Error`].
    pub fn parse(s: &str) -> Self {
        match s {
            "online" => Self::Online,
            "offline" => Self::Offline,
            "scanning" => Self::Scanning,
            _ => Self::Error,
        }
    }
}

/// A filesystem volume known to the index. Identified by its filesystem UUID so that it survives
/// changes of device name or mount point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeInfo {
    /// Filesystem UUID (e.g. from `/dev/disk/by-uuid`). For filesystems without a UUID a stable
    /// synthetic id of the form `dev:<major>:<minor>` is used.
    pub fs_uuid: String,
    /// Block device, e.g. `/dev/nvme0n1p2`.
    pub device: PathBuf,
    /// Current (or last known) mount point.
    pub mount_point: PathBuf,
    /// Filesystem label, if any.
    pub label: Option<String>,
    /// Filesystem type, e.g. `ext4`.
    pub fs_type: String,
    /// Current state.
    pub state: VolumeState,
    /// Unix time (ns) of the last completed full scan.
    pub last_full_scan_ns: Option<i64>,
    /// Duration of the last scan in milliseconds.
    pub last_scan_ms: Option<u64>,
}

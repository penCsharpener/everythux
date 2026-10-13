//! SQLite persistence (ROADMAP Phase 2). Schema in ROADMAP §4.

use std::path::Path;

use everythux_core::model::VolumeInfo;
use everythux_core::snapshot::IndexSnapshot;

use crate::IndexError;

/// Handle to `index.db`. One `Store` per thread; the indexer owns the single writer.
#[derive(Debug)]
pub struct Store {
    conn: rusqlite::Connection,
}

/// Summary numbers for `everythux-cli stats` and the status bar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreStats {
    /// Number of entries.
    pub entries: u64,
    /// Number of folders.
    pub dirs: u64,
    /// Database file size in bytes.
    pub db_bytes: u64,
    /// Per-volume (info, entry count).
    pub volumes: Vec<(VolumeInfo, u64)>,
}

impl Store {
    /// Opens (creating and migrating if needed) the database at `path`.
    pub fn open(path: &Path) -> Result<Self, IndexError> {
        let _ = path;
        todo!("Phase 2")
    }

    /// Loads the whole index into memory for searching.
    pub fn load_snapshot(&self) -> Result<IndexSnapshot, IndexError> {
        let _ = &self.conn;
        todo!("Phase 2")
    }

    /// Entry / folder / volume counts and database size.
    pub fn stats(&self) -> Result<StoreStats, IndexError> {
        todo!("Phase 2")
    }

    /// All known volumes (online and offline).
    pub fn volumes(&self) -> Result<Vec<VolumeInfo>, IndexError> {
        todo!("Phase 2")
    }
}

//! SQLite persistence (ROADMAP Phase 2). Schema in ROADMAP §4.
//!
//! # Deviations from the ROADMAP schema
//!
//! - `entries.child_dir` (nullable rowid of the `dirs` row a folder entry stands for). The
//!   snapshot loader needs the entry↔folder link, and a pre-assigned id lets the parallel walker
//!   emit a folder's listing before its parent's without any ordering constraint. Folders on other
//!   filesystems (mount points) have `NULL`.
//! - `dirs` has no table-level `UNIQUE(volume_id, path)`; the same unique index is created after
//!   a bulk load instead (a table constraint would force the index to exist during the load).
//! - No `entries_size` / `entries_mtime` indexes: the in-memory index does size and date
//!   filtering, and the two indexes cost about 30 of the roughly 160 bytes per entry (measured:
//!   131 B/entry without them, ROADMAP target about 150). `entries_dir`, `entries_ext`,
//!   `dirs_parent` and the unique `(volume_id, path)` index remain.
//! - A folder entry's `size` column holds the folder's recursive size (kept in sync with
//!   `dirs.total_size`); the walker stores `0`.
//! - Paths in `volumes` (`device`, `mount_point`) are `TEXT` when valid UTF-8, else `BLOB`.
//! - `volumes.state` is reconciled at startup ([`Store::sync_volumes`]); `scanning` is only
//!   reported through events, never persisted.
//!
//! # Concurrency
//!
//! One writer, many readers (WAL). A [`Store`] is used from one thread at a time; open another
//! one for reading from another thread or process.

mod apply;
mod exclusions;
mod idmap;
mod rebuild;
mod rename;
mod schema;
mod snapshot;
mod totals;
mod update;
mod volumes;

use std::path::{Path, PathBuf};

use everythux_core::model::{VolumeInfo, VolumeState};
use everythux_core::snapshot::IndexSnapshot;
use rusqlite::Connection;

use crate::IndexError;
use crate::discovery::DiscoveredVolume;

pub use rebuild::Rebuild;
pub use rename::RenameStats;
pub use update::{ApplyStats, DirRef, Update};

/// Handle to `index.db`. One `Store` per thread; the indexer owns the single writer.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
    path: PathBuf,
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

/// `<path>.<suffix>` (appending to the file name, whatever its extension).
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Turns SQLite's "this is not a database / is damaged" codes into [`IndexError::Corrupt`].
fn classify(path: &Path, err: rusqlite::Error) -> IndexError {
    use rusqlite::ErrorCode::{DatabaseCorrupt, NotADatabase};
    match &err {
        rusqlite::Error::SqliteFailure(e, _) if matches!(e.code, NotADatabase | DatabaseCorrupt) => {
            IndexError::Corrupt {
                path: path.to_owned(),
                message: err.to_string(),
            }
        }
        _ => IndexError::Sqlite(err),
    }
}

/// Opens the live connection: pragmas, optional integrity check, schema creation / migration.
fn open_connection(path: &Path, check: bool) -> Result<Connection, IndexError> {
    let mut conn = Connection::open(path).map_err(|e| classify(path, e))?;
    schema::configure_live(&conn).map_err(|e| classify(path, e))?;
    if check {
        let mut stmt = conn
            .prepare("PRAGMA quick_check")
            .map_err(|e| classify(path, e))?;
        let report: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .and_then(Iterator::collect)
            .map_err(|e| classify(path, e))?;
        if report.first().map(String::as_str) != Some("ok") {
            let message = report.iter().take(5).cloned().collect::<Vec<_>>().join("; ");
            tracing::error!(path = %path.display(), %message, "index database failed quick_check");
            return Err(IndexError::Corrupt {
                path: path.to_owned(),
                message,
            });
        }
    }
    schema::ensure_schema(&mut conn, path)?;
    Ok(conn)
}

impl Store {
    /// Opens (creating and migrating if needed) the database at `path`.
    ///
    /// Runs `PRAGMA quick_check`; a damaged file yields [`IndexError::Corrupt`] so the caller can
    /// offer to delete ([`Store::delete_database`]) and rebuild it.
    pub fn open(path: &Path) -> Result<Self, IndexError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| IndexError::io(parent, e))?;
        }
        Ok(Self {
            conn: open_connection(path, true)?,
            path: path.to_owned(),
        })
    }

    /// Deletes the database file and its `-wal`, `-shm` and `.building` companions. Missing files
    /// are fine. Use this to recover from [`IndexError::Corrupt`].
    pub fn delete_database(path: &Path) -> Result<(), IndexError> {
        for p in [
            path.to_owned(),
            sibling(path, "-wal"),
            sibling(path, "-shm"),
            sibling(path, ".building"),
        ] {
            match std::fs::remove_file(&p) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(IndexError::io(p, e)),
            }
        }
        Ok(())
    }

    /// The database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the whole index into memory for searching.
    pub fn load_snapshot(&self) -> Result<IndexSnapshot, IndexError> {
        snapshot::load(&self.conn)
    }

    /// Entry / folder / volume counts and database size.
    pub fn stats(&self) -> Result<StoreStats, IndexError> {
        let count = |sql: &str| -> rusqlite::Result<u64> {
            let n: i64 = self.conn.query_row(sql, [], |r| r.get(0))?;
            Ok(u64::try_from(n).unwrap_or(0))
        };
        let pages = count("PRAGMA page_count")?;
        let page_size = count("PRAGMA page_size")?;

        let mut per_volume: std::collections::HashMap<i64, u64> = std::collections::HashMap::new();
        {
            let mut stmt = self.conn.prepare_cached(
                "SELECT d.volume_id, COUNT(*) FROM entries e JOIN dirs d ON d.id = e.dir_id GROUP BY d.volume_id",
            )?;
            for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
                let (id, n) = row?;
                per_volume.insert(id, u64::try_from(n).unwrap_or(0));
            }
        }
        Ok(StoreStats {
            entries: count("SELECT COUNT(*) FROM entries")?,
            dirs: count("SELECT COUNT(*) FROM dirs")?,
            db_bytes: pages * page_size,
            volumes: volumes::list(&self.conn)?
                .into_iter()
                .map(|(id, v)| (v, per_volume.get(&id).copied().unwrap_or(0)))
                .collect(),
        })
    }

    /// All known volumes (online and offline).
    pub fn volumes(&self) -> Result<Vec<VolumeInfo>, IndexError> {
        Ok(volumes::list(&self.conn)?.into_iter().map(|(_, v)| v).collect())
    }

    /// Whether the index holds no entries (a fresh database).
    pub fn is_empty(&self) -> Result<bool, IndexError> {
        let any: bool = self
            .conn
            .query_row("SELECT EXISTS(SELECT 1 FROM entries)", [], |r| r.get(0))?;
        Ok(!any)
    }

    /// Records the currently mounted volumes: known ones are refreshed (device, mount point,
    /// label) and marked online, new ones are added, and volumes that are not mounted anymore
    /// become [`VolumeState::Offline`] with their entries kept. Returns all volumes.
    pub fn sync_volumes(&mut self, discovered: &[DiscoveredVolume]) -> Result<Vec<VolumeInfo>, IndexError> {
        let tx = self.conn.transaction()?;
        for v in discovered {
            volumes::upsert(&tx, v, VolumeState::Online)?;
        }
        let online: Vec<String> = discovered.iter().map(|v| v.fs_uuid.clone()).collect();
        volumes::mark_others_offline(&tx, &online)?;
        tx.commit()?;
        self.volumes()
    }

    /// Persists a volume's state (for example [`VolumeState::Error`] after a failed scan).
    pub fn set_volume_state(&mut self, fs_uuid: &str, state: VolumeState) -> Result<(), IndexError> {
        self.conn.execute(
            "UPDATE volumes SET state = ?1 WHERE fs_uuid = ?2",
            rusqlite::params![state.as_str(), fs_uuid],
        )?;
        Ok(())
    }

    /// Starts a full rebuild into `<db>.building`. Volumes in the database that are *not* in
    /// `rescanned` are carried over untouched (marked offline), so unmounted disks keep their
    /// entries. See [`Rebuild`].
    pub fn begin_rebuild(&self, rescanned: &[DiscoveredVolume]) -> Result<Rebuild, IndexError> {
        Rebuild::begin(&self.path, rescanned)
    }

    /// Starts one write transaction for incremental changes. See [`Update`].
    pub fn begin_update(&mut self) -> Result<Update<'_>, IndexError> {
        Update::begin(&mut self.conn)
    }

    /// Replaces the live database with the finished `building` file (called by
    /// [`Rebuild::finish`]).
    pub(crate) fn swap_in(&mut self, building: &Path) -> Result<(), IndexError> {
        // Move everything from the WAL into the main file first: the WAL is deleted below.
        if let Err(e) = self
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        {
            tracing::warn!(error = %e, "checkpoint before swapping the index failed");
        }
        let placeholder = Connection::open_in_memory()?;
        let old = std::mem::replace(&mut self.conn, placeholder);
        if let Err((_conn, e)) = old.close() {
            tracing::warn!(error = %e, "closing the old index connection failed");
        }
        for suffix in ["-wal", "-shm"] {
            let p = sibling(&self.path, suffix);
            if let Err(e) = std::fs::remove_file(&p)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return self.reopen_after(Err(IndexError::io(p, e)));
            }
        }
        let renamed = std::fs::rename(building, &self.path).map_err(|e| IndexError::io(&self.path, e));
        if renamed.is_ok()
            && let Some(dir) = self.path.parent().filter(|p| !p.as_os_str().is_empty())
        {
            // Make the rename itself durable.
            if let Err(e) = std::fs::File::open(dir).and_then(|d| d.sync_all()) {
                tracing::debug!(error = %e, "fsync of the index directory failed");
            }
        }
        self.reopen_after(renamed)
    }

    /// Reopens the live connection (after a swap attempt) and returns `outcome`.
    fn reopen_after(&mut self, outcome: Result<(), IndexError>) -> Result<(), IndexError> {
        self.conn = open_connection(&self.path, false)?;
        outcome
    }
}

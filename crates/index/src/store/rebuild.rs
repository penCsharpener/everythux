//! Full rebuild into `<db>.building` followed by an atomic swap (ROADMAP Phase 2).
//!
//! The building file is a plain, unjournaled database nobody else opens, so it is written with
//! `journal_mode=OFF` / `synchronous=OFF` and secondary indexes are created only after the bulk
//! load. A crash (or a dropped [`Rebuild`]) leaves the live database untouched; the half-written
//! building file is deleted by the next rebuild.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::Connection;

use super::apply::Applier;
use super::update::{ApplyStats, now_ns};
use super::{Store, schema, sibling, totals, volumes};
use crate::IndexError;
use crate::discovery::DiscoveredVolume;
use crate::walker::{IdAlloc, ScanBatch};
use everythux_core::model::VolumeState;

/// Rows between commits of the bulk load.
const COMMIT_EVERY_ROWS: usize = 100_000;

/// A rebuild in progress. Created by [`Store::begin_rebuild`].
///
/// Dropping it without calling [`Rebuild::finish`] abandons the build and deletes the
/// `.building` file; the live database is untouched either way.
#[derive(Debug)]
pub struct Rebuild {
    // Declared first: the connection closes before the files are deleted.
    conn: Connection,
    applier: Applier,
    ids: Arc<IdAlloc>,
    files: BuildingFiles,
}

/// The `.building` file and its SQLite companions; removes whatever still exists on drop. After a
/// successful swap the file has been renamed away, so there is nothing left to remove.
#[derive(Debug)]
struct BuildingFiles(PathBuf);

impl Drop for BuildingFiles {
    fn drop(&mut self) {
        for suffix in ["", "-journal", "-wal", "-shm"] {
            if let Err(e) = remove_if_exists(&sibling(&self.0, suffix)) {
                tracing::warn!(error = %e, "cannot remove aborted index build");
            }
        }
    }
}

fn remove_if_exists(path: &Path) -> Result<(), IndexError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(IndexError::io(path, e)),
    }
}

/// Copies volumes that are not being rescanned (unmounted disks) from the live database.
fn carry_over(conn: &Connection, live: &Path, rescanned: &[DiscoveredVolume]) -> Result<(), IndexError> {
    let live_str = live.to_str().ok_or_else(|| {
        IndexError::Internal(format!("database path {} is not valid UTF-8", live.display()))
    })?;
    conn.execute("ATTACH DATABASE ?1 AS old", [live_str])?;
    let result = (|| -> rusqlite::Result<()> {
        let has_old: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM old.sqlite_master WHERE name = 'volumes')",
            [],
            |r| r.get(0),
        )?;
        if !has_old {
            return Ok(());
        }
        conn.execute_batch("BEGIN; CREATE TEMP TABLE rescanned (fs_uuid TEXT PRIMARY KEY)")?;
        for v in rescanned {
            conn.execute("INSERT OR IGNORE INTO rescanned VALUES (?1)", [&v.fs_uuid])?;
        }
        conn.execute(
            "INSERT INTO volumes (id, fs_uuid, device, mount_point, label, fs_type, state, last_full_scan, last_scan_ms) \
             SELECT id, fs_uuid, device, mount_point, label, fs_type, ?1, last_full_scan, last_scan_ms \
             FROM old.volumes WHERE fs_uuid NOT IN (SELECT fs_uuid FROM rescanned)",
            [VolumeState::Offline.as_str()],
        )?;
        conn.execute_batch(
            "INSERT INTO dirs (id, volume_id, parent_id, path, inode, mtime_ns, total_size) \
               SELECT id, volume_id, parent_id, path, inode, mtime_ns, total_size \
               FROM old.dirs WHERE volume_id IN (SELECT id FROM volumes); \
             INSERT INTO entries (id, dir_id, name, name_fold, kind, ext, size, mtime_ns, inode, uid, mode, child_dir) \
               SELECT e.id, e.dir_id, e.name, e.name_fold, e.kind, e.ext, e.size, e.mtime_ns, e.inode, e.uid, e.mode, e.child_dir \
               FROM old.entries e JOIN dirs d ON d.id = e.dir_id; \
             DROP TABLE rescanned; COMMIT",
        )?;
        Ok(())
    })();
    if result.is_err() {
        // Best effort: leave the connection usable for the error path.
        let _ = conn.execute_batch("ROLLBACK");
    }
    let detached = conn.execute_batch("DETACH DATABASE old");
    result?;
    detached?;
    Ok(())
}

impl Rebuild {
    pub(super) fn begin(live: &Path, rescanned: &[DiscoveredVolume]) -> Result<Self, IndexError> {
        let building = sibling(live, ".building");
        for suffix in ["", "-journal", "-wal", "-shm"] {
            remove_if_exists(&sibling(&building, suffix))?;
        }
        let files = BuildingFiles(building);
        let conn = Connection::open(&files.0)?;
        schema::configure_building(&conn)?;
        schema::create_tables(&conn)?;
        carry_over(&conn, live, rescanned)?;
        let next: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM dirs", [], |r| r.get(0))?;
        conn.execute_batch("BEGIN")?;
        Ok(Self {
            conn,
            applier: Applier::default(),
            ids: Arc::new(IdAlloc::new(next)),
            files,
        })
    }

    /// Source of ids for folders (continues after any carried-over rows).
    pub fn ids(&self) -> Arc<IdAlloc> {
        Arc::clone(&self.ids)
    }

    /// Registers a volume that is about to be scanned and returns its row id.
    pub fn add_volume(&mut self, volume: &DiscoveredVolume) -> Result<i64, IndexError> {
        Ok(volumes::upsert(&self.conn, volume, VolumeState::Scanning)?)
    }

    /// Applies a batch of walker output for `volume_id` (all folders are new).
    pub fn apply(&mut self, volume_id: i64, batch: ScanBatch) -> Result<(), IndexError> {
        self.applier.apply_batch(&self.conn, volume_id, batch)?;
        // `finish` recomputes every folder size in one pass: no need to remember what was touched.
        self.applier.touched.clear();
        if self.applier.rows_since_commit >= COMMIT_EVERY_ROWS {
            self.conn.execute_batch("COMMIT; BEGIN")?;
            self.applier.rows_since_commit = 0;
        }
        Ok(())
    }

    /// Marks a volume as completely scanned.
    pub fn finish_volume(&mut self, volume_id: i64, elapsed_ms: u64) -> Result<(), IndexError> {
        Ok(volumes::record_scan(
            &self.conn,
            volume_id,
            true,
            elapsed_ms,
            now_ns(),
        )?)
    }

    /// Changes made so far.
    pub fn stats(&self) -> ApplyStats {
        ApplyStats {
            upserted: self.applier.upserted,
            removed: self.applier.removed,
        }
    }

    /// Creates the indexes, computes folder sizes, analyzes, and atomically replaces the live
    /// database held by `store`. Until the rename the previous index stays fully intact.
    pub fn finish(self, store: &mut Store) -> Result<ApplyStats, IndexError> {
        let stats = self.stats();
        let Self { conn, files, .. } = self;
        conn.execute_batch("COMMIT")?;
        schema::create_indexes(&conn)?;
        conn.execute_batch("BEGIN")?;
        totals::recompute_all(&conn)?;
        conn.execute_batch("COMMIT")?;
        conn.execute_batch("ANALYZE")?;
        conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES ('last_rebuild', CAST(strftime('%s','now') AS TEXT))",
            [],
        )?;
        conn.close().map_err(|(_, e)| IndexError::Sqlite(e))?;
        // The building file was written with synchronous=OFF: make it durable before it replaces
        // the live database.
        std::fs::File::open(&files.0)
            .and_then(|f| f.sync_all())
            .map_err(|e| IndexError::io(&files.0, e))?;
        // On failure `files` is dropped and deletes the leftover file.
        store.swap_in(&files.0)?;
        Ok(stats)
    }
}

//! Incremental changes to the live database inside one transaction.

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use everythux_core::exclusion::ExclusionSet;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use super::apply::{Applier, clear_subtree, subtree_bounds};
use super::{exclusions, totals, volumes};
use crate::IndexError;
use crate::walker::{IdAlloc, KnownRow, KnownTree, ScanBatch};

/// What an [`Update`] changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ApplyStats {
    /// Entries inserted or updated.
    pub upserted: u64,
    /// Entries deleted (including those inside deleted folders).
    pub removed: u64,
}

/// A folder row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirRef {
    /// Row id.
    pub id: i64,
    /// Row id of the containing folder, `None` for a volume root.
    pub parent_id: Option<i64>,
}

/// One write transaction on the live database: partial rescans, live updates and exclusions.
///
/// Nothing is visible to readers until [`Update::commit`]; dropping the value rolls everything
/// back. Foreign-key checks are deferred to commit, so folders may arrive in any order.
#[derive(Debug)]
pub struct Update<'s> {
    tx: Transaction<'s>,
    applier: Applier,
    ids: Arc<IdAlloc>,
}

impl<'s> Update<'s> {
    pub(super) fn begin(conn: &'s mut Connection) -> Result<Self, IndexError> {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        let next: i64 = tx.query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM dirs", [], |r| r.get(0))?;
        Ok(Self {
            tx,
            applier: Applier::default(),
            ids: Arc::new(IdAlloc::new(next)),
        })
    }

    /// Source of ids for folders the walker discovers during this transaction.
    pub fn ids(&self) -> Arc<IdAlloc> {
        Arc::clone(&self.ids)
    }

    /// Row id of the volume with this filesystem UUID.
    pub fn volume_id(&self, fs_uuid: &str) -> Result<Option<i64>, IndexError> {
        Ok(self
            .tx
            .query_row("SELECT id FROM volumes WHERE fs_uuid = ?1", [fs_uuid], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// The folder row at absolute `path` on a volume.
    pub fn find_dir(&self, volume_id: i64, path: &Path) -> Result<Option<DirRef>, IndexError> {
        Ok(self
            .tx
            .prepare_cached("SELECT id, parent_id FROM dirs WHERE volume_id = ?1 AND path = ?2")?
            .query_row(params![volume_id, path.as_os_str().as_bytes()], |r| {
                Ok(DirRef {
                    id: r.get(0)?,
                    parent_id: r.get(1)?,
                })
            })
            .optional()?)
    }

    /// The deepest stored folder that is `path` or one of its ancestors.
    pub fn nearest_known_dir(
        &self,
        volume_id: i64,
        path: &Path,
    ) -> Result<Option<(PathBuf, DirRef)>, IndexError> {
        for ancestor in path.ancestors() {
            if let Some(found) = self.find_dir(volume_id, ancestor)? {
                return Ok(Some((ancestor.to_owned(), found)));
            }
        }
        Ok(None)
    }

    /// Loads the stored folders at and below `root` (or just `root` and its direct sub-folders
    /// when `recursive` is false) for an incremental walk.
    pub fn known_tree(&self, volume_id: i64, root: &Path, recursive: bool) -> Result<KnownTree, IndexError> {
        let map = |r: &rusqlite::Row<'_>| {
            Ok(KnownRow {
                id: r.get(0)?,
                parent: r.get(1)?,
                path: r.get(2)?,
                mtime_ns: r.get(3)?,
            })
        };
        let rows: Vec<KnownRow> = if recursive {
            let root_bytes = root.as_os_str().as_bytes();
            let (lo, hi) = subtree_bounds(root_bytes);
            let mut stmt = self.tx.prepare_cached(
                "SELECT id, parent_id, path, mtime_ns FROM dirs \
                 WHERE volume_id = ?1 AND (path = ?2 OR (path >= ?3 AND path < ?4))",
            )?;
            stmt.query_map(params![volume_id, root_bytes, lo, hi], map)?
                .collect::<Result<_, _>>()?
        } else {
            let Some(dir) = self.find_dir(volume_id, root)? else {
                return Ok(KnownTree::new());
            };
            let mut stmt = self.tx.prepare_cached(
                "SELECT id, parent_id, path, mtime_ns FROM dirs WHERE id = ?1 OR parent_id = ?1",
            )?;
            stmt.query_map([dir.id], map)?.collect::<Result<_, _>>()?
        };
        Ok(KnownTree::build(rows))
    }

    /// Deletes all entries of folder `dir` and everything below it (the folder row itself stays).
    /// The caller follows up with a walk in `WalkMode::Fresh` whose `existing_root` is `dir`.
    pub fn prepare_replace(&mut self, dir: i64) -> Result<(), IndexError> {
        self.applier.removed += clear_subtree(&self.tx, dir, false)?;
        self.applier.touched.push(dir);
        Ok(())
    }

    /// Deletes a folder, everything below it and its entry in the parent.
    pub fn remove_dir(&mut self, dir: i64) -> Result<(), IndexError> {
        Ok(self.applier.remove_dir(&self.tx, dir)?)
    }

    /// Applies a batch of walker output for `volume_id`.
    pub fn apply(&mut self, volume_id: i64, batch: ScanBatch) -> Result<(), IndexError> {
        Ok(self.applier.apply_batch(&self.tx, volume_id, batch)?)
    }

    /// Deletes every entry (and folder subtree) matched by `exclusions`; no disk access.
    pub fn apply_exclusions(&mut self, rules: &ExclusionSet) -> Result<(), IndexError> {
        Ok(exclusions::apply(&self.tx, &mut self.applier, rules)?)
    }

    /// Records a finished scan of a volume (`full` also sets the last-full-scan time).
    pub fn record_scan(&mut self, volume_id: i64, full: bool, elapsed_ms: u64) -> Result<(), IndexError> {
        Ok(volumes::record_scan(
            &self.tx,
            volume_id,
            full,
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

    /// Recomputes recursive folder sizes of everything touched, then commits.
    pub fn commit(self) -> Result<ApplyStats, IndexError> {
        let stats = self.stats();
        totals::recompute(&self.tx, &self.applier.touched)?;
        self.tx.commit()?;
        Ok(stats)
    }
}

/// Unix time in nanoseconds.
pub(super) fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

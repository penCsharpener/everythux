//! Writing walker output into a database connection: plain inserts, folder diffs, subtree
//! deletes. Shared by the full-rebuild file and the live database's update transactions.

use std::collections::HashMap;

use everythux_core::model::EntryKind;
use rusqlite::{Connection, OptionalExtension, params};

use crate::walker::{DirListing, EntryRow, ListingKind, ScanBatch};

const INSERT_ENTRY: &str = "INSERT INTO entries \
    (dir_id, name, name_fold, kind, ext, size, mtime_ns, inode, uid, mode, child_dir) \
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)";

/// Clamps an unsigned value into SQLite's signed 64-bit integer.
pub(crate) fn db_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// The key range `[lo, hi)` covering every path strictly below `path` (raw bytes). Computed in
/// Rust and bound as BLOBs: SQL `||` would turn the operands into TEXT, which sorts differently.
pub(crate) fn subtree_bounds(path: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut base = path;
    while let [rest @ .., b'/'] = base {
        base = rest;
    }
    let mut lo = base.to_vec();
    lo.push(b'/');
    let mut hi = base.to_vec();
    hi.push(b'0'); // '0' is '/' + 1
    (lo, hi)
}

/// Accumulates what a series of batches changed.
#[derive(Debug, Default)]
pub(crate) struct Applier {
    /// Folders whose own entries changed: their recursive sizes (and their ancestors') are stale.
    pub(crate) touched: Vec<i64>,
    /// Entries inserted or updated.
    pub(crate) upserted: u64,
    /// Entries deleted (including those inside deleted sub-folders).
    pub(crate) removed: u64,
    /// Rows written since the caller last reset this (to pace commits).
    pub(crate) rows_since_commit: usize,
}

struct Stored {
    id: i64,
    kind: u8,
    size: i64,
    mtime_ns: i64,
    inode: i64,
    uid: i64,
    mode: i64,
    child_dir: Option<i64>,
}

impl Applier {
    /// Applies one batch of walker output for `volume_id`.
    pub(crate) fn apply_batch(
        &mut self,
        conn: &Connection,
        volume_id: i64,
        mut batch: ScanBatch,
    ) -> rusqlite::Result<()> {
        // Ids are handed out parent-first; applying in id order keeps the B-trees appending.
        batch.listings.sort_unstable_by_key(|l| l.id);
        self.rows_since_commit += batch.rows();
        for listing in &batch.listings {
            match listing.kind {
                ListingKind::Insert => {
                    insert_dir(conn, volume_id, listing)?;
                    self.insert_all(conn, listing)?;
                    self.touched.push(listing.id);
                }
                ListingKind::Reset => {
                    update_dir(conn, listing)?;
                    self.insert_all(conn, listing)?;
                    self.touched.push(listing.id);
                }
                ListingKind::Diff => {
                    update_dir(conn, listing)?;
                    if self.diff(conn, listing)? {
                        self.touched.push(listing.id);
                    }
                }
            }
        }
        for &id in &batch.vanished {
            self.remove_dir(conn, id)?;
        }
        Ok(())
    }

    fn insert_all(&mut self, conn: &Connection, listing: &DirListing) -> rusqlite::Result<()> {
        let mut stmt = conn.prepare_cached(INSERT_ENTRY)?;
        for row in &listing.entries {
            insert_entry(&mut stmt, listing.id, listing, row)?;
        }
        self.upserted += listing.entries.len() as u64;
        Ok(())
    }

    /// Makes the stored entries of `listing.id` equal to the listing. Returns whether anything
    /// changed.
    fn diff(&mut self, conn: &Connection, listing: &DirListing) -> rusqlite::Result<bool> {
        let mut stored: HashMap<Vec<u8>, Stored> = {
            let mut stmt = conn.prepare_cached(
                "SELECT id, name, kind, size, mtime_ns, inode, uid, mode, child_dir \
                 FROM entries WHERE dir_id = ?1",
            )?;
            stmt.query_map([listing.id], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(1)?,
                    Stored {
                        id: r.get(0)?,
                        kind: r.get(2)?,
                        size: r.get(3)?,
                        mtime_ns: r.get(4)?,
                        inode: r.get(5)?,
                        uid: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
                        mode: r.get::<_, Option<i64>>(7)?.unwrap_or(0),
                        child_dir: r.get(8)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<_>>()?
        };

        let mut changed = false;
        let mut insert = conn.prepare_cached(INSERT_ENTRY)?;
        let mut update = conn.prepare_cached(
            "UPDATE entries SET size = ?1, mtime_ns = ?2, inode = ?3, uid = ?4, mode = ?5, child_dir = ?6 \
             WHERE id = ?7",
        )?;
        for row in &listing.entries {
            let new_child = (row.child_dir != 0).then_some(row.child_dir);
            match stored.remove(listing.name(row)) {
                Some(old) if old.kind == row.kind as u8 => {
                    let is_dir = row.kind == EntryKind::Dir;
                    // A folder's stored size is its recursive size, maintained by the store.
                    let same = (is_dir || old.size == db_i64(row.size))
                        && old.mtime_ns == row.mtime_ns
                        && old.inode == db_i64(row.inode)
                        && old.uid == i64::from(row.uid)
                        && old.mode == i64::from(row.mode)
                        && old.child_dir == new_child;
                    if same {
                        continue;
                    }
                    if let Some(gone) = old.child_dir
                        && Some(gone) != new_child
                    {
                        self.removed += clear_subtree(conn, gone, true)?;
                    }
                    let size = if is_dir { old.size } else { db_i64(row.size) };
                    update.execute(params![
                        size,
                        row.mtime_ns,
                        db_i64(row.inode),
                        i64::from(row.uid),
                        i64::from(row.mode),
                        new_child,
                        old.id
                    ])?;
                    self.upserted += 1;
                    changed = true;
                }
                Some(old) => {
                    self.delete_stored(conn, &old)?;
                    insert_entry(&mut insert, listing.id, listing, row)?;
                    self.upserted += 1;
                    changed = true;
                }
                None => {
                    insert_entry(&mut insert, listing.id, listing, row)?;
                    self.upserted += 1;
                    changed = true;
                }
            }
        }
        for old in stored.values() {
            self.delete_stored(conn, old)?;
            changed = true;
        }
        Ok(changed)
    }

    fn delete_stored(&mut self, conn: &Connection, old: &Stored) -> rusqlite::Result<()> {
        conn.prepare_cached("DELETE FROM entries WHERE id = ?1")?
            .execute([old.id])?;
        self.removed += 1;
        if let Some(dir) = old.child_dir {
            self.removed += clear_subtree(conn, dir, true)?;
        }
        Ok(())
    }

    /// Removes a folder, everything below it and the entry that stands for it in its parent.
    pub(crate) fn remove_dir(&mut self, conn: &Connection, dir_id: i64) -> rusqlite::Result<()> {
        let parent: Option<i64> = conn
            .prepare_cached("SELECT parent_id FROM dirs WHERE id = ?1")?
            .query_row([dir_id], |r| r.get(0))
            .optional()?
            .flatten();
        if let Some(parent) = parent {
            let n = conn
                .prepare_cached("DELETE FROM entries WHERE dir_id = ?1 AND child_dir = ?2")?
                .execute([parent, dir_id])?;
            self.removed += n as u64;
            self.touched.push(parent);
        }
        self.removed += clear_subtree(conn, dir_id, true)?;
        Ok(())
    }
}

fn insert_dir(conn: &Connection, volume_id: i64, l: &DirListing) -> rusqlite::Result<()> {
    conn.prepare_cached(
        "INSERT INTO dirs (id, volume_id, parent_id, path, inode, mtime_ns) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?
    .execute(params![
        l.id,
        volume_id,
        l.parent_id,
        l.path.as_slice(),
        db_i64(l.inode),
        l.validator_mtime_ns()
    ])?;
    Ok(())
}

/// Refreshes a stored folder's own metadata and the entry that represents it in its parent.
fn update_dir(conn: &Connection, l: &DirListing) -> rusqlite::Result<()> {
    conn.prepare_cached("UPDATE dirs SET inode = ?1, mtime_ns = ?2 WHERE id = ?3")?
        .execute(params![db_i64(l.inode), l.validator_mtime_ns(), l.id])?;
    if let Some(parent) = l.parent_id {
        conn.prepare_cached(
            "UPDATE entries SET mtime_ns = ?1, inode = ?2 WHERE dir_id = ?3 AND child_dir = ?4",
        )?
        .execute(params![l.mtime_ns, db_i64(l.inode), parent, l.id])?;
    }
    Ok(())
}

fn insert_entry(
    stmt: &mut rusqlite::CachedStatement<'_>,
    dir_id: i64,
    listing: &DirListing,
    row: &EntryRow,
) -> rusqlite::Result<()> {
    let fold = std::str::from_utf8(listing.fold(row)).unwrap_or_default();
    let ext = std::str::from_utf8(listing.ext(row))
        .ok()
        .filter(|e| !e.is_empty());
    stmt.execute(params![
        dir_id,
        listing.name(row),
        fold,
        row.kind as u8,
        ext,
        db_i64(row.size),
        row.mtime_ns,
        db_i64(row.inode),
        i64::from(row.uid),
        i64::from(row.mode),
        (row.child_dir != 0).then_some(row.child_dir),
    ])?;
    Ok(())
}

/// Deletes the entries of folder `dir_id` and of everything below it, plus every folder row
/// below it; with `delete_root_row` also the folder's own row and its entry in the parent.
/// Returns the number of entries deleted.
pub(crate) fn clear_subtree(conn: &Connection, dir_id: i64, delete_root_row: bool) -> rusqlite::Result<u64> {
    let Some((volume_id, path, parent)) = conn
        .prepare_cached("SELECT volume_id, path, parent_id FROM dirs WHERE id = ?1")?
        .query_row([dir_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        })
        .optional()?
    else {
        return Ok(0);
    };
    let (lo, hi) = subtree_bounds(&path);
    let mut removed = conn
        .prepare_cached("DELETE FROM entries WHERE dir_id = ?1")?
        .execute([dir_id])? as u64;
    removed += conn
        .prepare_cached(
            "DELETE FROM entries WHERE dir_id IN \
             (SELECT id FROM dirs WHERE volume_id = ?1 AND path >= ?2 AND path < ?3)",
        )?
        .execute(params![volume_id, lo, hi])? as u64;
    conn.prepare_cached("DELETE FROM dirs WHERE volume_id = ?1 AND path >= ?2 AND path < ?3 AND id <> ?4")?
        .execute(params![volume_id, lo, hi, dir_id])?;
    if delete_root_row {
        conn.prepare_cached("DELETE FROM dirs WHERE id = ?1")?
            .execute([dir_id])?;
        if let Some(parent) = parent {
            removed += conn
                .prepare_cached("DELETE FROM entries WHERE dir_id = ?1 AND child_dir = ?2")?
                .execute([parent, dir_id])? as u64;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_cover_exactly_the_subtree() {
        let (lo, hi) = subtree_bounds(b"/a/b");
        assert_eq!((lo.as_slice(), hi.as_slice()), (&b"/a/b/"[..], &b"/a/b0"[..]));
        for (path, inside) in [
            (&b"/a/b/c"[..], true),
            (b"/a/b/c/d", true),
            (b"/a/b", false),
            (b"/a/bc", false),
            (b"/a/b.x", false),
            (b"/a/a", false),
        ] {
            assert_eq!(path >= lo.as_slice() && path < hi.as_slice(), inside, "{path:?}");
        }
        let (lo, hi) = subtree_bounds(b"/");
        assert_eq!((lo.as_slice(), hi.as_slice()), (&b"/"[..], &b"0"[..]));
    }
}

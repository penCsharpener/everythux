//! Renaming one entry in place: the index follows a rename that already happened on disk.

use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use everythux_core::text;
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::Store;
use super::apply::subtree_bounds;
use crate::IndexError;

/// What [`Store::rename_path`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenameStats {
    /// The index was updated. `false` means nothing was changed because the old path isn't
    /// indexed, or the new name is already taken in the index; the caller re-lists the parent.
    pub applied: bool,
    /// Folder rows (the renamed folder and everything below it) whose path was rewritten.
    pub dirs_rewritten: u64,
}

impl Store {
    /// Applies a rename of `from` to `to` (same parent folder) in one transaction, without
    /// touching the disk: the entry's `name`, `name_fold` and `ext` change and, for a folder,
    /// the `path` of its `dirs` row and of every folder below it. Ids, inodes, parents and
    /// `total_size` stay.
    ///
    /// Nothing changes (and `applied` is `false`) when `from` isn't indexed or `to` already
    /// exists in the index. Exclusion rules are the caller's business.
    pub fn rename_path(&mut self, from: &Path, to: &Path) -> Result<RenameStats, IndexError> {
        let invalid = |reason: &'static str| IndexError::InvalidRename {
            from: from.to_owned(),
            to: to.to_owned(),
            reason,
        };
        let (Some(parent), Some(old_name), Some(new_name)) =
            (from.parent(), from.file_name(), to.file_name())
        else {
            return Err(invalid("the path has no parent folder or file name"));
        };
        if to.parent() != Some(parent) {
            return Err(invalid("a rename must stay in the same folder"));
        }
        let (old_name, new_name) = (old_name.as_bytes(), new_name.as_bytes());
        if new_name.is_empty() || new_name.contains(&0) {
            return Err(invalid("the new name is empty or contains NUL"));
        }

        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let none = RenameStats::default();
        // Online volumes only: an offline volume may share the mount point.
        let Some((dir_id, volume_id)) = tx
            .prepare_cached(
                "SELECT d.id, d.volume_id FROM dirs d JOIN volumes v ON v.id = d.volume_id \
                 WHERE d.path = ?1 AND v.state <> 'offline'",
            )?
            .query_row([parent.as_os_str().as_bytes()], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })
            .optional()?
        else {
            return Ok(none);
        };
        let find = |name: &[u8]| -> rusqlite::Result<Option<(i64, Option<i64>)>> {
            tx.prepare_cached("SELECT id, child_dir FROM entries WHERE dir_id = ?1 AND name = ?2")?
                .query_row(params![dir_id, name], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()
        };
        let Some((entry_id, child_dir)) = find(old_name)? else {
            return Ok(none);
        };
        if find(new_name)?.is_some() {
            return Ok(none);
        }
        if child_dir.is_some() {
            let taken: bool = tx
                .prepare_cached("SELECT EXISTS(SELECT 1 FROM dirs WHERE volume_id = ?1 AND path = ?2)")?
                .query_row(params![volume_id, to.as_os_str().as_bytes()], |r| r.get(0))?;
            if taken {
                return Ok(none);
            }
        }

        let ext = text::extension(new_name);
        tx.prepare_cached("UPDATE entries SET name = ?1, name_fold = ?2, ext = ?3 WHERE id = ?4")?
            .execute(params![new_name, text::fold(new_name), ext.as_deref(), entry_id])?;

        let mut dirs_rewritten = 0;
        if let Some(child) = child_dir {
            // The prefix is rewritten in Rust: SQL `||` and `substr` would treat the BLOB paths
            // as text, which breaks on NUL-free but non-UTF-8 names.
            let (from_b, to_b) = (from.as_os_str().as_bytes(), to.as_os_str().as_bytes());
            let (lo, hi) = subtree_bounds(from_b);
            let rows: Vec<(i64, Vec<u8>)> = tx
                .prepare_cached(
                    "SELECT id, path FROM dirs WHERE volume_id = ?1 AND (id = ?2 OR (path >= ?3 AND path < ?4))",
                )?
                .query_map(params![volume_id, child, lo, hi], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?;
            let mut set = tx.prepare_cached("UPDATE dirs SET path = ?1 WHERE id = ?2")?;
            let mut path = Vec::new();
            for (id, old) in &rows {
                path.clear();
                path.extend_from_slice(to_b);
                path.extend_from_slice(old.get(from_b.len()..).unwrap_or_default());
                set.execute(params![path.as_slice(), id])?;
            }
            dirs_rewritten = rows.len() as u64;
        }
        tx.commit()?;
        Ok(RenameStats {
            applied: true,
            dirs_rewritten,
        })
    }
}

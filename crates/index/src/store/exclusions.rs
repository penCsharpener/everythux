//! Applying exclusion rules to what is already indexed.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use everythux_core::exclusion::ExclusionSet;
use rusqlite::Connection;

use super::apply::Applier;

struct Hit {
    entry: i64,
    dir: i64,
    child_dir: Option<i64>,
}

/// Deletes every entry matched by `rules`; matched folders take their whole subtree with them.
pub(crate) fn apply(conn: &Connection, applier: &mut Applier, rules: &ExclusionSet) -> rusqlite::Result<()> {
    if rules.is_empty() {
        return Ok(());
    }
    let mut dir_paths: HashMap<i64, Vec<u8>> = HashMap::new();
    {
        let mut stmt = conn.prepare_cached("SELECT id, path FROM dirs")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            dir_paths.insert(row.get(0)?, row.get(1)?);
        }
    }

    // Collect first: the table cannot be modified while it is being iterated.
    let mut hits: Vec<Hit> = Vec::new();
    let mut path: Vec<u8> = Vec::new();
    {
        let mut stmt = conn.prepare_cached("SELECT id, dir_id, name, kind, child_dir FROM entries")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let dir: i64 = row.get(1)?;
            let Some(dir_path) = dir_paths.get(&dir) else {
                continue;
            };
            path.clear();
            path.extend_from_slice(dir_path);
            if path.last() != Some(&b'/') {
                path.push(b'/');
            }
            path.extend_from_slice(row.get_ref(2)?.as_bytes()?);
            let is_dir = row.get_ref(3)?.as_i64()? == 1;
            if rules.is_excluded(Path::new(OsStr::from_bytes(&path)), is_dir) {
                hits.push(Hit {
                    entry: row.get(0)?,
                    dir,
                    child_dir: row.get(4)?,
                });
            }
        }
    }

    // Folders first, shallowest first: removing a folder makes the hits inside it no-ops.
    hits.sort_unstable_by_key(|h| {
        h.child_dir
            .map_or((1, 0), |c| (0, dir_paths.get(&c).map_or(0, Vec::len)))
    });
    let mut delete_entry = conn.prepare_cached("DELETE FROM entries WHERE id = ?1")?;
    for hit in hits {
        if let Some(child) = hit.child_dir {
            applier.remove_dir(conn, child)?;
        } else if delete_entry.execute([hit.entry])? > 0 {
            applier.removed += 1;
            applier.touched.push(hit.dir);
        }
    }
    Ok(())
}

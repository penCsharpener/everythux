//! Recursive folder sizes (`dirs.total_size`, and the `size` of the folder's own entry).

use std::collections::HashMap;

use rusqlite::Connection;

use super::idmap::IdMap;

/// Above this many stale folders a single pass over the whole database beats per-folder queries.
const BULK_THRESHOLD: usize = 4000;

/// Recomputes the sizes of `touched` folders and all their ancestors.
pub(crate) fn recompute(conn: &Connection, touched: &[i64]) -> rusqlite::Result<()> {
    if touched.is_empty() {
        return Ok(());
    }
    if touched.len() > BULK_THRESHOLD {
        return recompute_all(conn);
    }

    // Close over the ancestors, remembering each folder's path length: a parent's path is always
    // shorter than its children's, so descending length is a valid bottom-up order.
    let mut path_len: HashMap<i64, i64> = HashMap::with_capacity(touched.len() * 2);
    let mut stack = touched.to_vec();
    {
        let mut up = conn.prepare_cached("SELECT parent_id, length(path) FROM dirs WHERE id = ?1")?;
        while let Some(id) = stack.pop() {
            if path_len.contains_key(&id) {
                continue;
            }
            let found = up
                .query_row([id], |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?)))
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?;
            if let Some((parent, len)) = found {
                path_len.insert(id, len);
                stack.extend(parent);
            }
        }
    }
    if path_len.len() > BULK_THRESHOLD {
        return recompute_all(conn);
    }
    let mut order: Vec<(i64, i64)> = path_len.into_iter().collect();
    order.sort_unstable_by_key(|&(id, len)| (std::cmp::Reverse(len), id));

    let mut own =
        conn.prepare_cached("SELECT COALESCE(SUM(size), 0) FROM entries WHERE dir_id = ?1 AND kind <> 1")?;
    let mut kids =
        conn.prepare_cached("SELECT COALESCE(SUM(total_size), 0) FROM dirs WHERE parent_id = ?1")?;
    let mut set_dir = conn.prepare_cached("UPDATE dirs SET total_size = ?1 WHERE id = ?2")?;
    let mut set_entry = conn.prepare_cached(
        "UPDATE entries SET size = ?1 WHERE child_dir = ?2 AND dir_id = (SELECT parent_id FROM dirs WHERE id = ?2)",
    )?;
    for (id, _) in order {
        let files: i64 = own.query_row([id], |r| r.get(0))?;
        let subdirs: i64 = kids.query_row([id], |r| r.get(0))?;
        let total = files.saturating_add(subdirs);
        set_dir.execute(rusqlite::params![total, id])?;
        set_entry.execute(rusqlite::params![total, id])?;
    }
    Ok(())
}

/// Recomputes every folder's size in one pass over the database.
pub(crate) fn recompute_all(conn: &Connection) -> rusqlite::Result<()> {
    let (count, max_id): (i64, i64) =
        conn.query_row("SELECT COUNT(*), COALESCE(MAX(id), 0) FROM dirs", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    let count = usize::try_from(count).unwrap_or(0);
    let mut map = IdMap::new(max_id, count);
    let mut ids: Vec<i64> = Vec::with_capacity(count);
    let mut parent_of: Vec<Option<i64>> = Vec::with_capacity(count);
    {
        let mut stmt = conn.prepare_cached("SELECT id, parent_id FROM dirs ORDER BY id")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            map.insert(id, u32::try_from(ids.len()).unwrap_or(u32::MAX));
            ids.push(id);
            parent_of.push(row.get(1)?);
        }
    }
    let parent_idx: Vec<Option<u32>> = parent_of.iter().map(|p| p.and_then(|p| map.get(p))).collect();

    let mut total = vec![0_i64; ids.len()];
    {
        let mut stmt =
            conn.prepare_cached("SELECT dir_id, SUM(size) FROM entries WHERE kind <> 1 GROUP BY dir_id")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            if let Some(i) = map.get(row.get(0)?) {
                total[i as usize] = row.get(1)?;
            }
        }
    }

    // Kahn's algorithm: a folder is final once all its sub-folders have been added to it.
    let mut pending = vec![0_u32; ids.len()];
    for p in parent_idx.iter().flatten() {
        pending[*p as usize] += 1;
    }
    let mut ready: Vec<u32> = (0..ids.len())
        .filter(|&i| pending[i] == 0)
        .map(|i| u32::try_from(i).unwrap_or(u32::MAX))
        .collect();
    while let Some(i) = ready.pop() {
        if let Some(p) = parent_idx[i as usize] {
            total[p as usize] = total[p as usize].saturating_add(total[i as usize]);
            pending[p as usize] -= 1;
            if pending[p as usize] == 0 {
                ready.push(p);
            }
        }
    }

    {
        let mut set = conn.prepare_cached("UPDATE dirs SET total_size = ?1 WHERE id = ?2")?;
        for (id, total) in ids.iter().zip(&total) {
            set.execute(rusqlite::params![total, id])?;
        }
    }
    conn.execute(
        "UPDATE entries SET size = COALESCE((SELECT total_size FROM dirs WHERE dirs.id = entries.child_dir), 0) \
         WHERE kind = 1 AND child_dir IS NOT NULL",
        [],
    )?;
    Ok(())
}

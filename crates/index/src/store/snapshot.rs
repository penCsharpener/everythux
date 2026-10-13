//! `Store::load_snapshot`: the SQLite → in-memory conversion. Perf-critical (ROADMAP §9): two
//! sequential passes (folders, then entries), arenas sized up front, no per-entry allocation.

use std::collections::HashMap;

use everythux_core::ids::{DirId, EntryId, VolumeId};
use everythux_core::model::EntryKind;
use everythux_core::snapshot::{ByteArena, DirTable, EntryTable, IndexSnapshot};
use rusqlite::Connection;

use super::idmap::IdMap;
use super::volumes;
use crate::IndexError;

/// Folds names into `u16` ids. Names beyond `u16::MAX` distinct extensions share id 0.
struct ExtInterner {
    ids: HashMap<Box<str>, u16>,
    list: Vec<Box<str>>,
    last: Option<(Box<str>, u16)>,
}

impl ExtInterner {
    fn new() -> Self {
        Self {
            ids: HashMap::new(),
            list: vec![Box::from("")],
            last: None,
        }
    }

    fn intern(&mut self, ext: Option<&str>) -> u16 {
        let Some(ext) = ext.filter(|e| !e.is_empty()) else {
            return 0;
        };
        // Runs of equal extensions (`a.jpg b.jpg …`) are common.
        if let Some((last, id)) = &self.last
            && &**last == ext
        {
            return *id;
        }
        let id = if let Some(&id) = self.ids.get(ext) {
            id
        } else if let Ok(id) = u16::try_from(self.list.len()) {
            let boxed: Box<str> = Box::from(ext);
            self.ids.insert(boxed.clone(), id);
            self.list.push(boxed);
            id
        } else {
            0
        };
        self.last = Some((Box::from(ext), id));
        id
    }
}

fn count(conn: &Connection, sql: &str) -> rusqlite::Result<usize> {
    let n: i64 = conn.query_row(sql, [], |r| r.get(0))?;
    Ok(usize::try_from(n).unwrap_or(0))
}

pub(crate) fn load(conn: &Connection) -> Result<IndexSnapshot, IndexError> {
    let volume_rows = volumes::list(conn)?;
    let mut vol_map = IdMap::new(
        volume_rows.iter().map(|(id, _)| *id).max().unwrap_or(0),
        volume_rows.len(),
    );
    for (i, (id, _)) in volume_rows.iter().enumerate() {
        vol_map.insert(*id, u32::try_from(i).unwrap_or(u32::MAX));
    }

    let (dirs, dir_map, parent_ids) = load_dirs(conn, &vol_map)?;
    let mut snapshot = IndexSnapshot {
        volumes: volume_rows.into_iter().map(|(_, v)| v).collect(),
        dirs,
        entries: EntryTable::default(),
        extensions: Vec::new(),
    };
    // Parents are resolved after the id map is complete (a parent's rowid may exceed its child's).
    snapshot.dirs.parent = parent_ids
        .iter()
        .map(|p| p.and_then(|p| dir_map.get(p)).map_or(DirId::NONE, DirId))
        .collect();

    load_entries(conn, &mut snapshot, &dir_map)?;
    Ok(snapshot)
}

type DirsLoaded = (DirTable, IdMap, Vec<Option<i64>>);

fn load_dirs(conn: &Connection, vol_map: &IdMap) -> Result<DirsLoaded, IndexError> {
    let n = count(conn, "SELECT COUNT(*) FROM dirs")?;
    let max_id: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) FROM dirs", [], |r| r.get(0))?;
    let path_bytes = count(conn, "SELECT COALESCE(SUM(length(path)), 0) FROM dirs")?;

    let mut map = IdMap::new(max_id, n);
    let mut t = DirTable {
        paths: ByteArena::with_capacity(n, path_bytes),
        parent: Vec::new(),
        volume: Vec::with_capacity(n),
        total_size: Vec::with_capacity(n),
        mtime_ns: Vec::with_capacity(n),
        entry: Vec::with_capacity(n),
    };
    let mut parents = Vec::with_capacity(n);

    let mut stmt = conn.prepare_cached(
        "SELECT id, volume_id, parent_id, path, mtime_ns, total_size FROM dirs ORDER BY id",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id = row.get_ref(0)?.as_i64()?;
        let Some(volume) = vol_map.get(row.get_ref(1)?.as_i64()?) else {
            tracing::warn!(dir = id, "folder of an unknown volume skipped");
            continue;
        };
        map.insert(id, u32::try_from(t.volume.len()).unwrap_or(u32::MAX));
        parents.push(row.get_ref(2)?.as_i64_or_null()?);
        t.paths.push(row.get_ref(3)?.as_bytes()?);
        t.volume.push(VolumeId(volume));
        t.mtime_ns.push(row.get_ref(4)?.as_i64()?);
        t.total_size
            .push(u64::try_from(row.get_ref(5)?.as_i64_or_null()?.unwrap_or(0)).unwrap_or(0));
        t.entry.push(EntryId::NONE);
    }
    Ok((t, map, parents))
}

fn load_entries(conn: &Connection, snap: &mut IndexSnapshot, dir_map: &IdMap) -> Result<(), IndexError> {
    let n = count(conn, "SELECT COUNT(*) FROM entries")?;
    // Estimate arena sizes from a sample instead of a second full pass.
    let (avg_name, avg_fold): (f64, f64) = conn.query_row(
        "SELECT COALESCE(AVG(length(name)), 16), COALESCE(AVG(length(name_fold)), 16) \
         FROM (SELECT name, name_fold FROM entries LIMIT 20000)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let guess = |avg: f64| ((avg + 1.0) * n as f64 * 1.05) as usize;
    let mut e = EntryTable {
        names: ByteArena::with_capacity(n, guess(avg_name)),
        names_fold: ByteArena::with_capacity(n, guess(avg_fold)),
        dir: Vec::with_capacity(n),
        kind: Vec::with_capacity(n),
        size: Vec::with_capacity(n),
        mtime_ns: Vec::with_capacity(n),
        ext: Vec::with_capacity(n),
        self_dir: Vec::with_capacity(n),
    };
    let mut exts = ExtInterner::new();
    let mut orphans = 0_u64;

    let mut stmt = conn.prepare_cached(
        "SELECT dir_id, name, name_fold, kind, ext, size, mtime_ns, child_dir FROM entries ORDER BY id",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let Some(dir) = dir_map.get(row.get_ref(0)?.as_i64()?) else {
            orphans += 1;
            continue;
        };
        let idx = u32::try_from(e.dir.len()).unwrap_or(u32::MAX);
        let kind = EntryKind::from_u8(u8::try_from(row.get_ref(3)?.as_i64()?).unwrap_or(u8::MAX));
        let mut size = u64::try_from(row.get_ref(5)?.as_i64()?).unwrap_or(0);
        let mut self_dir = DirId::NONE;
        if kind.is_dir()
            && let Some(child) = row.get_ref(7)?.as_i64_or_null()?
            && let Some(d) = dir_map.get(child)
        {
            self_dir = DirId(d);
            size = snap.dirs.total_size[d as usize];
            snap.dirs.entry[d as usize] = EntryId(idx);
        }
        e.names.push(row.get_ref(1)?.as_bytes()?);
        e.names_fold.push(row.get_ref(2)?.as_bytes()?);
        e.dir.push(DirId(dir));
        e.kind.push(kind);
        e.size.push(size);
        e.mtime_ns.push(row.get_ref(6)?.as_i64()?);
        e.ext.push(exts.intern(row.get_ref(4)?.as_str_or_null()?));
        e.self_dir.push(self_dir);
    }
    if orphans > 0 {
        tracing::warn!(
            orphans,
            "entries of unknown folders skipped while loading the snapshot"
        );
    }
    snap.entries = e;
    snap.extensions = exts.list;
    Ok(())
}

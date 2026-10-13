//! The `volumes` table.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use everythux_core::model::{VolumeInfo, VolumeState};
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::discovery::DiscoveredVolume;

pub(crate) const VOLUME_COLUMNS: &str =
    "id, fs_uuid, device, mount_point, label, fs_type, state, last_full_scan, last_scan_ms";

/// Paths are stored as TEXT when valid UTF-8 (readable with the `sqlite3` shell), else as BLOB.
fn text_or_blob(bytes: &[u8]) -> Value {
    std::str::from_utf8(bytes).map_or_else(|_| Value::Blob(bytes.to_vec()), |s| Value::Text(s.to_owned()))
}

/// Reads a TEXT-or-BLOB column as raw bytes (mount points can be non-UTF-8).
fn bytes_at(row: &Row<'_>, idx: usize) -> rusqlite::Result<Option<Vec<u8>>> {
    Ok(match row.get_ref(idx)? {
        ValueRef::Text(b) | ValueRef::Blob(b) => Some(b.to_vec()),
        _ => None,
    })
}

/// Maps a row selected with [`VOLUME_COLUMNS`] (id is column 0 and is returned separately).
pub(crate) fn volume_from_row(row: &Row<'_>) -> rusqlite::Result<(i64, VolumeInfo)> {
    let path = |idx| -> rusqlite::Result<PathBuf> {
        Ok(bytes_at(row, idx)?
            .map(|b| PathBuf::from(OsString::from_vec(b)))
            .unwrap_or_default())
    };
    let info = VolumeInfo {
        fs_uuid: row.get(1)?,
        device: path(2)?,
        mount_point: path(3)?,
        label: row.get(4)?,
        fs_type: row.get(5)?,
        state: VolumeState::parse(&row.get::<_, String>(6)?),
        last_full_scan_ns: row.get(7)?,
        last_scan_ms: row
            .get::<_, Option<i64>>(8)?
            .map(|v| u64::try_from(v).unwrap_or(0)),
    };
    Ok((row.get(0)?, info))
}

/// All volumes ordered by id, with their row ids.
pub(crate) fn list(conn: &Connection) -> rusqlite::Result<Vec<(i64, VolumeInfo)>> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {VOLUME_COLUMNS} FROM volumes ORDER BY id"))?;
    stmt.query_map([], volume_from_row)?.collect()
}

/// Inserts or refreshes a discovered volume (marking it online) and returns its row id.
pub(crate) fn upsert(conn: &Connection, v: &DiscoveredVolume, state: VolumeState) -> rusqlite::Result<i64> {
    let existing: Option<i64> = conn
        .prepare_cached("SELECT id FROM volumes WHERE fs_uuid = ?1")?
        .query_row([&v.fs_uuid], |r| r.get(0))
        .optional()?;
    let device = text_or_blob(v.device.as_os_str().as_bytes());
    let mount = text_or_blob(v.mount_point.as_os_str().as_bytes());
    if let Some(id) = existing {
        conn.prepare_cached(
            "UPDATE volumes SET device = ?1, mount_point = ?2, label = ?3, fs_type = ?4, state = ?5 WHERE id = ?6",
        )?
        .execute(params![device, mount, v.label, v.fs_type, state.as_str(), id])?;
        Ok(id)
    } else {
        conn.prepare_cached(
            "INSERT INTO volumes (fs_uuid, device, mount_point, label, fs_type, state) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?
        .execute(params![
            v.fs_uuid,
            device,
            mount,
            v.label,
            v.fs_type,
            state.as_str()
        ])?;
        Ok(conn.last_insert_rowid())
    }
}

/// Marks every volume whose uuid is not in `online` as offline.
pub(crate) fn mark_others_offline(conn: &Connection, online: &[String]) -> rusqlite::Result<()> {
    for (id, info) in list(conn)? {
        if !online.contains(&info.fs_uuid) && info.state != VolumeState::Offline {
            conn.prepare_cached("UPDATE volumes SET state = ?1 WHERE id = ?2")?
                .execute(params![VolumeState::Offline.as_str(), id])?;
        }
    }
    Ok(())
}

/// Records a finished scan.
pub(crate) fn record_scan(
    conn: &Connection,
    id: i64,
    full: bool,
    elapsed_ms: u64,
    now_ns: i64,
) -> rusqlite::Result<()> {
    let ms = i64::try_from(elapsed_ms).unwrap_or(i64::MAX);
    if full {
        conn.prepare_cached(
            "UPDATE volumes SET last_full_scan = ?1, last_scan_ms = ?2, state = 'online' WHERE id = ?3",
        )?
        .execute(params![now_ns, ms, id])?;
    } else {
        conn.prepare_cached("UPDATE volumes SET last_scan_ms = ?1, state = 'online' WHERE id = ?2")?
            .execute(params![ms, id])?;
    }
    Ok(())
}

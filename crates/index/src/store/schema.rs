//! SQL schema, pragmas and migrations.

use rusqlite::{Connection, OptionalExtension};

use crate::IndexError;

/// Highest schema version this build understands.
pub(crate) const SCHEMA_VERSION: u32 = 1;

/// Tables only. Secondary indexes are created separately so a bulk load can add them last.
const TABLES: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS volumes (
  id              INTEGER PRIMARY KEY,
  fs_uuid         TEXT NOT NULL UNIQUE,
  device          TEXT,
  mount_point     TEXT,
  label           TEXT,
  fs_type         TEXT NOT NULL,
  state           TEXT NOT NULL,
  last_full_scan  INTEGER,
  last_scan_ms    INTEGER
);

CREATE TABLE IF NOT EXISTS dirs (
  id          INTEGER PRIMARY KEY,
  volume_id   INTEGER NOT NULL REFERENCES volumes(id) ON DELETE CASCADE,
  parent_id   INTEGER REFERENCES dirs(id) ON DELETE CASCADE,
  path        BLOB NOT NULL,
  inode       INTEGER NOT NULL,
  mtime_ns    INTEGER NOT NULL,
  total_size  INTEGER
);

CREATE TABLE IF NOT EXISTS entries (
  id          INTEGER PRIMARY KEY,
  dir_id      INTEGER NOT NULL REFERENCES dirs(id) ON DELETE CASCADE,
  name        BLOB NOT NULL,
  name_fold   TEXT NOT NULL,
  kind        INTEGER NOT NULL,
  ext         TEXT,
  size        INTEGER NOT NULL,
  mtime_ns    INTEGER NOT NULL,
  inode       INTEGER NOT NULL,
  uid         INTEGER,
  mode        INTEGER,
  child_dir   INTEGER
);
";

const INDEXES: &str = "
CREATE UNIQUE INDEX IF NOT EXISTS dirs_volume_path ON dirs(volume_id, path);
CREATE INDEX IF NOT EXISTS dirs_parent   ON dirs(parent_id);
CREATE INDEX IF NOT EXISTS entries_dir   ON entries(dir_id);
CREATE INDEX IF NOT EXISTS entries_ext   ON entries(ext);
";

/// Pragmas for the long-lived live connection.
pub(crate) fn configure_live(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "cache_size", -65_536)?;
    Ok(())
}

/// Pragmas for the `.building` file: nothing in it matters until it is complete and renamed.
pub(crate) fn configure_building(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "OFF")?;
    conn.pragma_update(None, "synchronous", "OFF")?;
    conn.pragma_update(None, "foreign_keys", "OFF")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "cache_size", -262_144)?;
    Ok(())
}

/// Creates the tables (no secondary indexes) and the `meta` rows.
pub(crate) fn create_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(TABLES)?;
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES ('schema_version', ?1)",
        [SCHEMA_VERSION.to_string()],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO meta(key, value) VALUES ('created_at', CAST(strftime('%s','now') AS TEXT))",
        [],
    )?;
    Ok(())
}

/// Creates the secondary indexes (idempotent).
pub(crate) fn create_indexes(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(INDEXES)
}

/// Whether `conn` already contains any table at all.
fn has_any_table(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%')",
        [],
        |r| r.get(0),
    )
}

/// The `schema_version` stored in `meta`, if the file is an everythux index.
fn stored_version(conn: &Connection) -> rusqlite::Result<Option<u32>> {
    let has_meta: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')",
        [],
        |r| r.get(0),
    )?;
    if !has_meta {
        return Ok(None);
    }
    let value: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(value.and_then(|v| v.parse().ok()))
}

/// Creates a fresh schema or migrates an existing one up to [`SCHEMA_VERSION`].
pub(crate) fn ensure_schema(conn: &mut Connection, path: &std::path::Path) -> Result<(), IndexError> {
    match stored_version(conn)? {
        None if !has_any_table(conn)? => {
            let tx = conn.transaction()?;
            create_tables(&tx)?;
            create_indexes(&tx)?;
            tx.commit()?;
            Ok(())
        }
        None => Err(IndexError::Corrupt {
            path: path.to_owned(),
            message: "not an everythux index (no schema_version)".to_owned(),
        }),
        Some(found) if found > SCHEMA_VERSION => Err(IndexError::SchemaTooNew {
            found,
            supported: SCHEMA_VERSION,
        }),
        Some(found) => migrate(conn, found),
    }
}

/// Runs the numbered migrations from `from` up to [`SCHEMA_VERSION`]. There are none yet; the
/// shape is: one `match` arm per step, each in its own transaction, bumping `schema_version`.
fn migrate(conn: &mut Connection, from: u32) -> Result<(), IndexError> {
    let mut version = from;
    while version < SCHEMA_VERSION {
        let tx = conn.transaction()?;
        // Version 1 is the first schema. Future steps go here, keyed on `version`:
        //     if version == 1 { tx.execute_batch("ALTER TABLE …")?; }
        tracing::warn!(version, "no migration defined for schema version");
        version += 1;
        tx.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
            [version.to_string()],
        )?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_schema_is_created_once_and_versioned() {
        let mut conn = Connection::open_in_memory().expect("open");
        ensure_schema(&mut conn, std::path::Path::new(":memory:")).expect("create");
        assert_eq!(stored_version(&conn).expect("version"), Some(SCHEMA_VERSION));
        ensure_schema(&mut conn, std::path::Path::new(":memory:")).expect("idempotent");
    }

    #[test]
    fn foreign_database_is_rejected() {
        let mut conn = Connection::open_in_memory().expect("open");
        conn.execute_batch("CREATE TABLE other(x)").expect("create");
        let err = ensure_schema(&mut conn, std::path::Path::new("x.db")).expect_err("must fail");
        assert!(matches!(err, IndexError::Corrupt { .. }), "{err:?}");
    }

    #[test]
    fn newer_schema_is_rejected() {
        let mut conn = Connection::open_in_memory().expect("open");
        create_tables(&conn).expect("tables");
        conn.execute("UPDATE meta SET value = '999' WHERE key = 'schema_version'", [])
            .expect("update");
        let err = ensure_schema(&mut conn, std::path::Path::new("x.db")).expect_err("must fail");
        assert!(
            matches!(err, IndexError::SchemaTooNew { found: 999, .. }),
            "{err:?}"
        );
    }
}

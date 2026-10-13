use std::path::{Path, PathBuf};

/// Errors from the indexer and the store.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IndexError {
    /// SQLite failure.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// I/O failure on a specific path.
    #[error("I/O error on {path:?}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The database was written by a newer, incompatible version.
    #[error("index database schema version {found} is newer than supported ({supported})")]
    SchemaTooNew {
        /// Version in the file.
        found: u32,
        /// Highest version this build understands.
        supported: u32,
    },
    /// No indexed volume contains the requested path / uuid.
    #[error("not part of any indexed volume: {0}")]
    UnknownVolume(String),
    /// The job was cancelled.
    #[error("cancelled")]
    Cancelled,
    /// The database file is damaged (`PRAGMA quick_check` failed) or isn't an everythux index.
    /// The app should offer to delete it ([`crate::Store::delete_database`]) and rebuild.
    #[error("index database {path:?} is corrupt: {message}")]
    Corrupt {
        /// The database file.
        path: PathBuf,
        /// What the integrity check reported.
        message: String,
    },
    /// An internal invariant failed (for example a worker thread panicked).
    #[error("internal error: {0}")]
    Internal(String),
}

impl IndexError {
    /// Builds an [`IndexError::Io`] for `path`.
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Converts a `rustix` errno into an [`IndexError::Io`] for `path`.
    pub(crate) fn errno(path: &Path, errno: rustix::io::Errno) -> Self {
        Self::io(path, errno.into())
    }
}

impl From<rusqlite::types::FromSqlError> for IndexError {
    fn from(err: rusqlite::types::FromSqlError) -> Self {
        Self::Sqlite(err.into())
    }
}

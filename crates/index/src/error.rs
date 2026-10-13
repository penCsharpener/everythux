use std::path::PathBuf;

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
}

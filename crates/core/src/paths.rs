//! XDG locations used by everythux.

use std::path::PathBuf;

/// Where config, database and logs live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    /// `$XDG_CONFIG_HOME/everythux/config.json`
    pub config_file: PathBuf,
    /// `history.json` next to the config file.
    pub history_file: PathBuf,
    /// `dirty-folders.list` next to the config file: folders touched by in-app file operations
    /// while automatic indexing is off.
    pub dirty_file: PathBuf,
    /// `$XDG_DATA_HOME/everythux/index.db`
    pub db_file: PathBuf,
    /// `$XDG_STATE_HOME/everythux/` (falls back to the data dir)
    pub log_dir: PathBuf,
}

impl AppPaths {
    /// Resolves the standard XDG locations. Returns `None` if no home directory can be determined.
    ///
    /// `EVERYTHUX_CONFIG` and `EVERYTHUX_DB` override the config and database paths (used by tests
    /// and for running several instances side by side).
    pub fn discover() -> Option<Self> {
        let dirs = directories::ProjectDirs::from("", "", "everythux")?;
        let config_file = std::env::var_os("EVERYTHUX_CONFIG")
            .map_or_else(|| dirs.config_dir().join("config.json"), PathBuf::from);
        let db_file =
            std::env::var_os("EVERYTHUX_DB").map_or_else(|| dirs.data_dir().join("index.db"), PathBuf::from);
        let log_dir = dirs.state_dir().unwrap_or_else(|| dirs.data_dir()).to_path_buf();
        Some(Self {
            history_file: config_file.with_file_name("history.json"),
            dirty_file: config_file.with_file_name("dirty-folders.list"),
            config_file,
            db_file,
            log_dir,
        })
    }

    /// Paths rooted in a single directory (tests, portable mode).
    pub fn in_dir(dir: &std::path::Path) -> Self {
        Self {
            config_file: dir.join("config.json"),
            history_file: dir.join("history.json"),
            dirty_file: dir.join("dirty-folders.list"),
            db_file: dir.join("index.db"),
            log_dir: dir.join("logs"),
        }
    }
}

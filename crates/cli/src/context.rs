//! Resolved file locations and shared helpers for opening the config and the database.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use everythux_core::config::Config;
use everythux_core::paths::AppPaths;
use everythux_index::Store;

/// Where the config file and the index database live for this invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Env {
    pub(crate) db: PathBuf,
    pub(crate) config: PathBuf,
}

impl Env {
    /// Explicit `--db` / `--config` win over the XDG defaults.
    pub(crate) fn resolve(db: Option<PathBuf>, config: Option<PathBuf>) -> Result<Self> {
        let discovered = AppPaths::discover();
        let pick = |explicit: Option<PathBuf>, default: fn(&AppPaths) -> &Path, flag: &str| {
            explicit
                .or_else(|| discovered.as_ref().map(|p| default(p).to_owned()))
                .with_context(|| {
                    format!("cannot determine a default location (no home directory); pass {flag}")
                })
        };
        Ok(Self {
            db: pick(db, |p| &p.db_file, "--db")?,
            config: pick(config, |p| &p.config_file, "--config")?,
        })
    }

    /// Loads the config (defaults when the file doesn't exist).
    pub(crate) fn load_config(&self) -> Result<Config> {
        Config::load(&self.config).context("cannot load the configuration")
    }

    /// Writes the config atomically.
    pub(crate) fn save_config(&self, config: &Config) -> Result<()> {
        config.save(&self.config).context("cannot save the configuration")
    }

    /// Opens the database, which must already exist (read-only commands must not create one).
    pub(crate) fn open_existing_store(&self) -> Result<Store> {
        if !self.db.exists() {
            bail!(
                "no index database at {}; run `everythux-cli reindex --full` first",
                self.db.display()
            );
        }
        Store::open(&self.db).with_context(|| format!("cannot open the index database {}", self.db.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_paths_win() {
        let env = Env::resolve(Some("/a/i.db".into()), Some("/a/c.json".into())).unwrap();
        assert_eq!(env.db, Path::new("/a/i.db"));
        assert_eq!(env.config, Path::new("/a/c.json"));
    }

    #[test]
    fn missing_database_gives_a_hint() {
        let env = Env {
            db: "/nonexistent-dir/index.db".into(),
            config: "/nonexistent-dir/config.json".into(),
        };
        let err = env.open_existing_store().unwrap_err().to_string();
        assert!(err.contains("reindex --full"), "{err}");
    }
}

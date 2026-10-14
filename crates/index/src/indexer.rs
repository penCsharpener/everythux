//! The background indexer: a worker thread that runs [`Job`]s one at a time, owns the single
//! SQLite writer and publishes fresh [`IndexSnapshot`]s.

mod jobs;
mod shared;
mod worker;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, unbounded};
use everythux_core::config::{IndexerConfig, StartupRescan, VolumesConfig};
use everythux_core::exclusion::ExclusionRule;
use everythux_core::model::VolumeInfo;
use everythux_core::snapshot::IndexSnapshot;

use self::shared::Shared;
use self::worker::{Worker, compile_lenient};
use crate::IndexError;
use crate::store::Store;
use crate::walker::make_pool;

/// How thoroughly to rescan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScanMode {
    /// Re-list only folders whose mtime changed since the last scan (ext4 bumps a folder's mtime
    /// when entries are added, removed or renamed). Misses in-place file size/mtime changes.
    Quick,
    /// Re-list and re-stat everything.
    Full,
}

/// Work for the indexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// Rebuild the whole database from scratch into `index.db.building`, then atomically swap
    /// (`Ctrl+Shift+R`). The previous index stays searchable until the swap.
    FullReindex,
    /// Rescan every enabled volume (startup / `F5` / scheduled).
    RescanAll(ScanMode),
    /// Rescan one volume.
    RescanVolume {
        /// Filesystem UUID or mount point.
        volume: String,
        /// Thoroughness.
        mode: ScanMode,
    },
    /// Rescan one folder subtree.
    RescanPath {
        /// Absolute folder path.
        path: PathBuf,
        /// Thoroughness.
        mode: ScanMode,
    },
    /// Delete every indexed entry matched by the current exclusions (no disk access).
    ApplyExclusions,
    /// Re-list exactly these folders non-recursively (live-update batches from the watcher).
    RefreshDirs(Vec<PathBuf>),
}

/// Identifies a submitted job in events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(pub u64);

/// Progress of the running job (sent at most ~10×/s).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Progress {
    /// Volume being scanned (mount point), if any.
    pub volume: Option<PathBuf>,
    /// Folders listed so far.
    pub dirs: u64,
    /// Entries seen so far.
    pub entries: u64,
    /// Unreadable folders / failed stats so far.
    pub errors: u64,
    /// A recently visited folder, for display.
    pub current: PathBuf,
    /// Rough completion estimate 0.0–1.0 based on the previous scan's entry count, if known.
    pub fraction: Option<f32>,
}

/// Result numbers of a finished job.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobStats {
    /// Entries added or updated.
    pub upserted: u64,
    /// Entries removed.
    pub removed: u64,
    /// Folders listed.
    pub dirs_listed: u64,
    /// Errors (unreadable folders etc.).
    pub errors: u64,
    /// Wall time.
    pub elapsed: Duration,
}

/// Events from the indexer to the UI / CLI.
#[derive(Debug, Clone)]
pub enum IndexEvent {
    /// A job started. `description` is human-readable ("Rescanning /home (quick)").
    Started {
        /// Job.
        job: JobId,
        /// Description.
        description: String,
    },
    /// Progress of the running job.
    Progress {
        /// Job.
        job: JobId,
        /// Numbers.
        progress: Progress,
    },
    /// A new snapshot is available (after any job that changed the index, and once at startup).
    SnapshotReady(Arc<IndexSnapshot>),
    /// The set or state of volumes changed.
    VolumesChanged(Vec<VolumeInfo>),
    /// A job finished successfully.
    Finished {
        /// Job.
        job: JobId,
        /// Numbers.
        stats: JobStats,
    },
    /// A job was cancelled.
    Cancelled {
        /// Job.
        job: JobId,
    },
    /// A job failed.
    Failed {
        /// Job.
        job: JobId,
        /// Error message (with causes).
        error: String,
    },
}

/// Everything the indexer needs from the config.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IndexerSettings {
    /// Volume selection.
    pub volumes: VolumesConfig,
    /// Exclusion rules (compiled by the indexer).
    pub exclusions: Vec<ExclusionRule>,
    /// Behaviour.
    pub indexer: IndexerConfig,
    /// Folders the live-update watcher must never watch, in addition to the database folder:
    /// the app's own config and log folders, whose writes would otherwise come back as change
    /// events and trigger rescans (and fresh snapshots) every time a setting is saved.
    pub watch_ignore: Vec<PathBuf>,
}

/// Handle to the background indexer thread. Dropping it shuts the thread down.
#[derive(Debug)]
pub struct Indexer {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl Indexer {
    /// Opens/creates the database at `db_path`, starts the worker thread and returns the handle
    /// plus the event stream. Immediately emits `SnapshotReady` with the stored index (possibly
    /// empty) and `VolumesChanged`, then runs `settings.indexer.rescan_on_startup` (an empty
    /// database always gets a full scan). Starts the inotify watcher if `live_updates` is on.
    ///
    /// Every job emits `Started`, throttled `Progress`, and one of `Finished` / `Cancelled` /
    /// `Failed`; a job that changed data publishes `SnapshotReady` *before* its `Finished`.
    /// Live-update jobs (`Job::RefreshDirs`) are rate limited: their snapshot may follow later.
    ///
    /// # Errors
    /// Opening the database can fail: [`IndexError::Corrupt`] (offer to rebuild, see
    /// [`Store::delete_database`]), [`IndexError::SchemaTooNew`], I/O or SQLite errors.
    pub fn start(
        db_path: &Path,
        settings: IndexerSettings,
    ) -> Result<(Self, Receiver<IndexEvent>), IndexError> {
        let store = Store::open(db_path)?;
        let shared = Arc::new(Shared::new());

        // Queued before the thread starts, so the startup scan precedes anything the caller submits.
        let startup = if store.is_empty()? {
            Some(Job::FullReindex)
        } else {
            match settings.indexer.rescan_on_startup {
                StartupRescan::None => None,
                StartupRescan::Quick => Some(Job::RescanAll(ScanMode::Quick)),
                StartupRescan::Full => Some(Job::RescanAll(ScanMode::Full)),
            }
        };
        if let Some(job) = startup {
            shared.submit(job);
        }

        let (events, receiver) = unbounded();
        let now = Instant::now();
        let worker = Worker {
            shared: Arc::clone(&shared),
            store,
            events,
            exclusions: compile_lenient(&settings.exclusions),
            pool: make_pool(settings.indexer.threads),
            settings,
            watcher: None,
            volumes_sent: Vec::new(),
            dirty: false,
            last_publish: now,
            last_load: Duration::ZERO,
            last_scheduled: now,
        };
        let handle = std::thread::Builder::new()
            .name("everythux-indexer".to_owned())
            .spawn(move || worker.run())
            .map_err(|e| IndexError::io(db_path, e))?;
        Ok((
            Self {
                shared,
                worker: Some(handle),
            },
            receiver,
        ))
    }

    /// Queues a job; jobs run one at a time in submission order. A `FullReindex` submitted while
    /// another `FullReindex` is queued or running is coalesced (the existing id is returned).
    /// Consecutive queued `RefreshDirs` jobs are merged.
    pub fn submit(&self, job: Job) -> JobId {
        self.shared.submit(job)
    }

    /// Cancels the running job (if any). Queued jobs stay queued.
    pub fn cancel_current(&self) {
        self.shared.cancel_current();
    }

    /// Replaces volume selection / exclusions / behaviour for subsequent jobs. Invalid exclusion
    /// rules are logged and skipped.
    pub fn update_settings(&self, settings: IndexerSettings) {
        self.shared.update_settings(settings);
    }

    /// The most recently published snapshot (empty until the first `SnapshotReady`).
    pub fn snapshot(&self) -> Arc<IndexSnapshot> {
        self.shared.snapshot()
    }
}

impl Drop for Indexer {
    fn drop(&mut self) {
        self.shared.shutdown();
        if let Some(handle) = self.worker.take()
            && handle.join().is_err()
        {
            tracing::error!("indexer thread panicked");
        }
    }
}

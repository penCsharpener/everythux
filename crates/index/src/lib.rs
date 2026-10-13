//! Indexing for everythux: volume discovery, the parallel walker, the SQLite store and the
//! background [`Indexer`] that runs jobs (full reindex, per-volume / per-folder rescans,
//! applying exclusions, live updates).
//!
//! The public items re-exported here are the contract used by `everythux-app` and
//! `everythux-cli`. Add to it freely; change existing signatures only with a strong reason.

pub mod discovery;
mod error;
pub mod indexer;
pub mod store;
pub mod walker;
pub mod watch;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

pub use discovery::{DiscoveredVolume, MountInfoLine, discover_volumes, parse_mountinfo};
pub use error::IndexError;
pub use indexer::{IndexEvent, Indexer, IndexerSettings, Job, JobId, JobStats, Progress, ScanMode};
pub use store::{ApplyStats, DirRef, Rebuild, Store, StoreStats, Update};

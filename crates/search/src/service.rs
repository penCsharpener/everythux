//! Background search thread.

use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Receiver;
use everythux_core::ids::EntryId;
use everythux_core::snapshot::IndexSnapshot;

use crate::engine::Sort;
use crate::query::QueryOptions;

/// What to search for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    /// Search box text.
    pub text: String,
    /// Switches.
    pub options: QueryOptions,
    /// Ordering.
    pub sort: Sort,
    /// Result cap (0 = unlimited).
    pub limit: usize,
}

/// Results of one request.
#[derive(Debug, Clone)]
pub struct SearchResults {
    /// Generation returned by [`SearchService::submit`]; the UI ignores results older than the
    /// latest generation it submitted.
    pub generation: u64,
    /// Matching entries in display order. Indexes into `snapshot`.
    pub ids: Arc<[EntryId]>,
    /// The snapshot `ids` refer to.
    pub snapshot: Arc<IndexSnapshot>,
    /// Entries in the snapshot (for "N of M objects").
    pub total_entries: usize,
    /// Time spent searching.
    pub elapsed: Duration,
    /// Parse error; when set, `ids` holds the previous results unchanged.
    pub error: Option<String>,
}

/// Handle to the search thread. Dropping it stops the thread.
#[derive(Debug)]
pub struct SearchService {
    _private: (),
}

impl SearchService {
    /// Starts the thread with an initial snapshot. Results arrive on the returned channel.
    pub fn start(snapshot: Arc<IndexSnapshot>) -> (Self, Receiver<SearchResults>) {
        let _ = snapshot;
        todo!("Phase 3")
    }

    /// Queues a request, cancelling any running one; returns its generation. Rapid submissions
    /// are coalesced (only the latest runs).
    pub fn submit(&self, request: SearchRequest) -> u64 {
        let _ = request;
        todo!()
    }

    /// Swaps in a new snapshot (sort permutations are rebuilt in the background; the last
    /// request is re-run against the new snapshot when ready).
    pub fn set_snapshot(&self, snapshot: Arc<IndexSnapshot>) {
        let _ = snapshot;
        todo!()
    }
}

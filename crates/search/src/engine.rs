//! The in-memory search engine.

use std::sync::Arc;

use everythux_core::config::ColumnId;
use everythux_core::ids::EntryId;
use everythux_core::snapshot::IndexSnapshot;

use crate::query::Query;

/// Result ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    /// Column.
    pub column: ColumnId,
    /// Ascending?
    pub ascending: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self {
            column: ColumnId::Name,
            ascending: true,
        }
    }
}

/// A snapshot plus per-column sort permutations, ready to answer queries.
///
/// Building it (sorting 2M entries per column, in parallel) happens off the UI thread.
#[derive(Debug)]
pub struct SearchIndex {
    snapshot: Arc<IndexSnapshot>,
}

impl SearchIndex {
    /// Builds the sort permutations for `snapshot`.
    pub fn build(snapshot: Arc<IndexSnapshot>) -> Self {
        let _ = snapshot;
        todo!("Phase 3")
    }

    /// The snapshot the results refer to.
    pub fn snapshot(&self) -> &Arc<IndexSnapshot> {
        &self.snapshot
    }

    /// Runs `query`, returning matching entries in `sort` order, or `None` if `cancelled()`
    /// became true (checked about every 64k entries). `limit` = 0 means unlimited.
    pub fn search(
        &self,
        query: &Query,
        sort: Sort,
        limit: usize,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Option<Vec<EntryId>> {
        let _ = (query, sort, limit, cancelled);
        todo!("Phase 3")
    }
}

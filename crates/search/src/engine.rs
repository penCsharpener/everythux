//! The in-memory search engine.

mod sort;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use everythux_core::config::ColumnId;
use everythux_core::ids::EntryId;
use everythux_core::snapshot::{ByteArena, IndexSnapshot};
use everythux_core::text::fold;
use rayon::prelude::*;

use crate::matcher::{CHUNK_ENTRIES, CHUNK_WORDS, Chunk, Plan, Scratch};
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

/// Entries per emission chunk when walking a permutation.
const EMIT_CHUNK: usize = 1 << 16;
/// Emission chunks processed per parallel wave (the limit is checked between waves).
const EMIT_WAVE: usize = 8;

/// A snapshot plus per-column sort permutations, ready to answer queries.
///
/// Building it (sorting 2M entries per column, in parallel) happens off the UI thread. The name
/// order is built eagerly; the other columns on first use (or via [`SearchIndex::warm`]).
#[derive(Debug)]
pub struct SearchIndex {
    snapshot: Arc<IndexSnapshot>,
    by_name: Vec<EntryId>,
    by_path: OnceLock<Vec<EntryId>>,
    by_type: OnceLock<Vec<EntryId>>,
    by_size: OnceLock<Vec<EntryId>>,
    by_modified: OnceLock<Vec<EntryId>>,
    /// Folded folder paths, for case-insensitive `path:` matching.
    dir_fold: OnceLock<ByteArena>,
}

impl SearchIndex {
    /// Builds the sort permutations for `snapshot`.
    pub fn build(snapshot: Arc<IndexSnapshot>) -> Self {
        let by_name = sort::name_permutation(&snapshot);
        Self {
            snapshot,
            by_name,
            by_path: OnceLock::new(),
            by_type: OnceLock::new(),
            by_size: OnceLock::new(),
            by_modified: OnceLock::new(),
            dir_fold: OnceLock::new(),
        }
    }

    /// The snapshot the results refer to.
    pub fn snapshot(&self) -> &Arc<IndexSnapshot> {
        &self.snapshot
    }

    /// All entry ids in `column` order (ascending), building the permutation on first use.
    pub fn permutation(&self, column: ColumnId) -> &[EntryId] {
        let cell = match column {
            ColumnId::Name => return &self.by_name,
            ColumnId::Path => &self.by_path,
            ColumnId::Type => &self.by_type,
            ColumnId::Size => &self.by_size,
            ColumnId::Modified => &self.by_modified,
        };
        cell.get_or_init(|| sort::column_permutation(&self.snapshot, &self.by_name, column, self.dir_fold()))
    }

    /// Builds the permutations of all columns now instead of on first use.
    pub fn warm(&self) {
        for column in ColumnId::ALL {
            self.permutation(column);
        }
    }

    /// Heap bytes held by the sort permutations built so far (excludes the snapshot).
    pub fn permutation_heap_size(&self) -> usize {
        let perms = [&self.by_path, &self.by_type, &self.by_size, &self.by_modified];
        let lazy: usize = perms.iter().filter_map(|c| c.get()).map(Vec::capacity).sum();
        (self.by_name.capacity() + lazy) * size_of::<EntryId>()
            + self.dir_fold.get().map_or(0, ByteArena::heap_size)
    }

    pub(crate) fn dir_fold(&self) -> &ByteArena {
        self.dir_fold.get_or_init(|| {
            let dirs = &self.snapshot.dirs.paths;
            let folded: Vec<String> = (0..dirs.len())
                .into_par_iter()
                .map(|i| fold(dirs.get(i)))
                .collect();
            let mut out = ByteArena::with_capacity(dirs.len(), dirs.bytes().len());
            for path in &folded {
                out.push(path.as_bytes());
            }
            out
        })
    }

    /// Runs `query`, returning matching entries in `sort` order, or `None` if `cancelled()`
    /// became true (checked about every 32k entries). `limit` = 0 means unlimited.
    pub fn search(
        &self,
        query: &Query,
        sort: Sort,
        limit: usize,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Option<Vec<EntryId>> {
        if cancelled() {
            return None;
        }
        let snap = &*self.snapshot;
        let n = snap.len();
        if n == 0 {
            return Some(Vec::new());
        }
        let perm = self.permutation(sort.column);
        let plan = Plan::bind(query.expr(), self);
        let words = match plan {
            Plan::All => return Some(emit_all(perm, sort.ascending, limit)),
            Plan::Nothing => return Some(Vec::new()),
            plan => evaluate(&plan, snap, cancelled)?,
        };
        emit(perm, &words, sort.ascending, limit, cancelled)
    }
}

/// Evaluates `plan` over all chunks in parallel into one global bitmask.
fn evaluate(
    plan: &Plan<'_>,
    snap: &IndexSnapshot,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<u64>> {
    let n = snap.len();
    let mut words = vec![0u64; n.div_ceil(64)];
    let aborted = AtomicBool::new(false);
    words
        .par_chunks_mut(CHUNK_WORDS)
        .enumerate()
        .for_each_init(Scratch::default, |sc, (index, out)| {
            if aborted.load(Ordering::Relaxed) {
                return;
            }
            if cancelled() {
                aborted.store(true, Ordering::Relaxed);
                return;
            }
            let start = index * CHUNK_ENTRIES;
            let ch = Chunk {
                start,
                len: CHUNK_ENTRIES.min(n - start),
            };
            let cand = ch.full_mask(sc);
            let hits = plan.eval(snap, ch, &cand, sc);
            out.copy_from_slice(&hits);
            sc.give(cand);
            sc.give(hits);
        });
    (!aborted.load(Ordering::Relaxed)).then_some(words)
}

fn emit_all(perm: &[EntryId], ascending: bool, limit: usize) -> Vec<EntryId> {
    let take = if limit == 0 {
        perm.len()
    } else {
        limit.min(perm.len())
    };
    if ascending {
        perm[..take].to_vec()
    } else {
        perm.iter().rev().take(take).copied().collect()
    }
}

/// Walks the permutation and keeps the entries whose bit is set, in parallel waves of chunks.
fn emit(
    perm: &[EntryId],
    words: &[u64],
    ascending: bool,
    limit: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<EntryId>> {
    let matches: usize = words.iter().map(|w| w.count_ones() as usize).sum();
    let wanted = if limit == 0 { matches } else { matches.min(limit) };
    let mut out = Vec::with_capacity(wanted);
    if wanted == 0 {
        return Some(out);
    }
    let hit = |id: EntryId| words[id.index() >> 6] >> (id.index() & 63) & 1 != 0;
    let chunks: Vec<&[EntryId]> = if ascending {
        perm.chunks(EMIT_CHUNK).collect()
    } else {
        perm.chunks(EMIT_CHUNK).rev().collect()
    };
    for wave in chunks.chunks(EMIT_WAVE) {
        if cancelled() {
            return None;
        }
        let parts: Vec<Vec<EntryId>> = wave
            .par_iter()
            .map(|chunk| {
                if ascending {
                    chunk.iter().copied().filter(|&id| hit(id)).collect()
                } else {
                    chunk.iter().rev().copied().filter(|&id| hit(id)).collect()
                }
            })
            .collect();
        for part in parts {
            out.extend(part);
        }
        if out.len() >= wanted {
            out.truncate(wanted);
            break;
        }
    }
    Some(out)
}

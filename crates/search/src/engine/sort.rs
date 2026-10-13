//! Sort permutations: for each column, all entry ids in display order.

use everythux_core::config::ColumnId;
use everythux_core::ids::EntryId;
use everythux_core::snapshot::{ByteArena, IndexSnapshot};
use rayon::prelude::*;

/// Name order: folded name, then raw name (so case variants sort deterministically), then id.
///
/// Comparing arena slices is a cache miss per comparison, so the first 16 folded bytes are copied
/// into the sort key; the arena is only consulted when two names share a 16 byte prefix.
pub(super) fn name_permutation(snap: &IndexSnapshot) -> Vec<EntryId> {
    #[derive(Clone, Copy)]
    struct Key {
        head: [u64; 2],
        id: u32,
    }
    let e = &snap.entries;
    let mut keys: Vec<Key> = (0..e.len())
        .into_par_iter()
        .map(|i| {
            let name = e.names_fold.get(i);
            let mut buf = [0u8; 16];
            let n = name.len().min(16);
            buf[..n].copy_from_slice(&name[..n]);
            // Names can't contain NUL, so zero padding sorts shorter names first, like slices do.
            let head = [
                u64::from_be_bytes(buf[..8].try_into().unwrap_or_default()),
                u64::from_be_bytes(buf[8..].try_into().unwrap_or_default()),
            ];
            Key { head, id: i as u32 }
        })
        .collect();
    keys.par_sort_unstable_by(|a, b| {
        a.head
            .cmp(&b.head)
            .then_with(|| {
                e.names_fold
                    .get(a.id as usize)
                    .cmp(e.names_fold.get(b.id as usize))
                    .then_with(|| e.names.get(a.id as usize).cmp(e.names.get(b.id as usize)))
            })
            .then(a.id.cmp(&b.id))
    });
    keys.into_par_iter().map(|k| EntryId(k.id)).collect()
}

/// Sorts entries by a `u64` key, ties in name order. `by_name` is the name permutation; building
/// the pairs in that order and sorting by `(key, position)` makes the result a stable sort.
fn keyed_permutation(by_name: &[EntryId], key: impl Fn(usize) -> u64 + Sync) -> Vec<EntryId> {
    let mut pairs: Vec<(u64, u32, u32)> = by_name
        .par_iter()
        .enumerate()
        .map(|(pos, id)| (key(id.index()), pos as u32, id.0))
        .collect();
    pairs.par_sort_unstable();
    pairs.into_par_iter().map(|(_, _, id)| EntryId(id)).collect()
}

/// Dense rank of each string in sorted order; equal strings share a rank.
fn ranks<'a>(count: usize, get: impl Fn(usize) -> &'a [u8] + Sync) -> Vec<u32> {
    let mut order: Vec<u32> = (0..count as u32).collect();
    order.par_sort_unstable_by(|&a, &b| get(a as usize).cmp(get(b as usize)));
    let mut rank = vec![0u32; count];
    let mut current = 0u32;
    for (n, &id) in order.iter().enumerate() {
        if n > 0 && get(order[n - 1] as usize) != get(id as usize) {
            current += 1;
        }
        rank[id as usize] = current;
    }
    rank
}

/// Builds the permutation for a non-name column.
pub(super) fn column_permutation(
    snap: &IndexSnapshot,
    by_name: &[EntryId],
    column: ColumnId,
    dir_fold: &ByteArena,
) -> Vec<EntryId> {
    let e = &snap.entries;
    match column {
        ColumnId::Name => by_name.to_vec(),
        ColumnId::Path => {
            let rank = ranks(snap.dirs.len(), |d| dir_fold.get(d));
            keyed_permutation(by_name, |i| u64::from(rank[e.dir[i].index()]))
        }
        ColumnId::Type => {
            let rank = ranks(snap.extensions.len(), |x| snap.extensions[x].as_bytes());
            keyed_permutation(by_name, |i| u64::from(rank[usize::from(e.ext[i])]))
        }
        ColumnId::Size => keyed_permutation(by_name, |i| e.size[i]),
        ColumnId::Modified => keyed_permutation(by_name, |i| (e.mtime_ns[i] as u64) ^ (1 << 63)),
    }
}

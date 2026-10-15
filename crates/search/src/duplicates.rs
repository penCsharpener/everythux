//! Duplicate detection within a result list.

use std::collections::HashMap;

use everythux_core::ids::EntryId;
use everythux_core::model::EntryKind;
use everythux_core::snapshot::IndexSnapshot;

/// Keeps the files of `ids` that have at least one other file in `ids` with the same name (which
/// includes the extension) and the same size, in their original order. Folders and other
/// non-files are dropped: only files can be duplicates.
pub fn retain_duplicates(snapshot: &IndexSnapshot, ids: Vec<EntryId>) -> Vec<EntryId> {
    let is_file = |id: EntryId| snapshot.entries.kind[id.index()] == EntryKind::File;
    let key = |id: EntryId| (snapshot.name(id), snapshot.entries.size[id.index()]);
    let mut counts: HashMap<(&[u8], u64), u32> = HashMap::with_capacity(ids.len() / 2);
    for &id in ids.iter().filter(|&&id| is_file(id)) {
        *counts.entry(key(id)).or_insert(0) += 1;
    }
    ids.into_iter()
        .filter(|&id| is_file(id) && counts.get(&key(id)).is_some_and(|&n| n > 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic::SnapshotBuilder;

    #[test]
    fn keeps_files_sharing_name_and_size_in_order() {
        let mut b = SnapshotBuilder::new();
        let v = b.volume("/", None);
        let root = b.dir("/", v);
        let a = b.dir("/a", v);
        let c = b.dir("/c", v);
        let one = b.file(a, b"x.txt", 10, 1);
        let lone = b.file(a, b"only.txt", 10, 1);
        let other_size = b.file(a, b"y.txt", 10, 1);
        let two = b.file(c, b"x.txt", 10, 1);
        let diff = b.file(c, b"y.txt", 11, 1);
        let snap = b.finish();
        let all = vec![one, lone, other_size, two, diff];
        let _ = root;
        assert_eq!(retain_duplicates(&snap, all), [one, two]);
    }
}

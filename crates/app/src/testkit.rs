//! Hand-built snapshots and results for tests.

use std::sync::Arc;
use std::time::Duration;

use everythux_core::ids::{DirId, EntryId, VolumeId};
use everythux_core::model::{EntryKind, VolumeInfo, VolumeState};
use everythux_core::snapshot::IndexSnapshot;
use everythux_core::text;

use crate::model::ResultsView;

/// Builds an [`IndexSnapshot`] without touching SQLite.
pub(crate) struct SnapshotBuilder {
    snap: IndexSnapshot,
}

impl SnapshotBuilder {
    /// A snapshot with one online volume mounted at `/`.
    pub(crate) fn new() -> Self {
        let mut snap = IndexSnapshot::empty();
        snap.volumes.push(VolumeInfo {
            fs_uuid: "uuid-root".to_owned(),
            device: "/dev/sda1".into(),
            mount_point: "/".into(),
            label: None,
            fs_type: "ext4".to_owned(),
            state: VolumeState::Online,
            last_full_scan_ns: Some(1_700_000_000_000_000_000),
            last_scan_ms: Some(1200),
        });
        Self { snap }
    }

    /// Adds a folder (and its entry in the parent folder).
    pub(crate) fn dir(&mut self, path: &str, parent: Option<DirId>) -> DirId {
        let id = DirId(self.snap.dirs.len() as u32);
        let d = &mut self.snap.dirs;
        d.paths.push(path.as_bytes());
        d.parent.push(parent.unwrap_or(DirId::NONE));
        d.volume.push(VolumeId(0));
        d.total_size.push(0);
        d.mtime_ns.push(1_700_000_000_000_000_000);
        d.entry.push(EntryId::NONE);
        if let Some(parent) = parent {
            let name = path.rsplit('/').next().unwrap_or(path);
            let entry = self.push_entry(
                parent,
                name.as_bytes(),
                EntryKind::Dir,
                0,
                1_700_000_000_000_000_000,
            );
            self.snap.entries.self_dir[entry.index()] = id;
            self.snap.dirs.entry[id.index()] = entry;
        }
        id
    }

    /// Adds a regular file.
    pub(crate) fn file(&mut self, dir: DirId, name: &str, size: u64, mtime_ns: i64) -> EntryId {
        self.push_entry(dir, name.as_bytes(), EntryKind::File, size, mtime_ns)
    }

    fn push_entry(&mut self, dir: DirId, name: &[u8], kind: EntryKind, size: u64, mtime_ns: i64) -> EntryId {
        let ext = if kind == EntryKind::File {
            text::extension(name).map(std::borrow::Cow::into_owned)
        } else {
            None
        };
        let ext_idx = match ext {
            Some(e) => {
                let pos = self.snap.extensions.iter().position(|x| **x == *e);
                pos.unwrap_or_else(|| {
                    self.snap.extensions.push(e.into_boxed_str());
                    self.snap.extensions.len() - 1
                }) as u16
            }
            None => 0,
        };
        let e = &mut self.snap.entries;
        let id = EntryId(e.len() as u32);
        e.names.push(name);
        e.names_fold.push(text::fold(name).as_bytes());
        e.dir.push(dir);
        e.kind.push(kind);
        e.size.push(size);
        e.mtime_ns.push(mtime_ns);
        e.ext.push(ext_idx);
        e.self_dir.push(DirId::NONE);
        id
    }

    /// Computes recursive folder sizes and freezes the snapshot.
    pub(crate) fn finish(mut self) -> Arc<IndexSnapshot> {
        let n_entries = self.snap.entries.len();
        for i in 0..n_entries {
            if self.snap.entries.kind[i].is_dir() {
                continue;
            }
            let size = self.snap.entries.size[i];
            let mut dir = self.snap.entries.dir[i];
            while !dir.is_none() {
                self.snap.dirs.total_size[dir.index()] += size;
                dir = self.snap.dirs.parent[dir.index()];
            }
        }
        for i in 0..n_entries {
            let dir = self.snap.entries.self_dir[i];
            if !dir.is_none() {
                self.snap.entries.size[i] = self.snap.dirs.total_size[dir.index()];
            }
        }
        Arc::new(self.snap)
    }
}

/// Results listing exactly `ids` of `snapshot`.
pub(crate) fn results_of(snapshot: &Arc<IndexSnapshot>, ids: &[EntryId]) -> ResultsView {
    ResultsView {
        ids: ids.into(),
        snapshot: Arc::clone(snapshot),
        total_entries: snapshot.len(),
        elapsed: Duration::from_millis(3),
    }
}

/// The standard fixture:
///
/// ```text
/// /home/me                 (dir 0)
/// /home/me/docs            (dir 1)  report.pdf (2 KiB), notes.txt (10 B)
/// /home/me/proj            (dir 2)
/// /home/me/proj/node_modules (dir 3)  junk.js (500 B), more.js (1 KiB)
/// /home/me/proj/bin        (dir 4)  tool (300 B)
/// /usr/bin                 (dir 5)  ls (100 B)
/// ```
pub(crate) struct Fixture {
    pub(crate) snapshot: Arc<IndexSnapshot>,
    pub(crate) report: EntryId,
    pub(crate) notes: EntryId,
    pub(crate) junk: EntryId,
    pub(crate) docs_entry: EntryId,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let mut b = SnapshotBuilder::new();
        let home = b.dir("/home/me", None);
        let docs = b.dir("/home/me/docs", Some(home));
        let proj = b.dir("/home/me/proj", Some(home));
        let nm = b.dir("/home/me/proj/node_modules", Some(proj));
        let bin = b.dir("/home/me/proj/bin", Some(proj));
        let usr_bin = b.dir("/usr/bin", None);
        let t = 1_700_000_000_000_000_000;
        let report = b.file(docs, "report.pdf", 2048, t);
        let notes = b.file(docs, "notes.txt", 10, t + 60_000_000_000);
        let junk = b.file(nm, "junk.js", 500, t);
        b.file(nm, "more.js", 1024, t);
        b.file(bin, "tool", 300, t);
        b.file(usr_bin, "ls", 100, t);
        let snapshot = b.finish();
        let docs_entry = snapshot.dirs.entry[docs.index()];
        Self {
            snapshot,
            report,
            notes,
            junk,
            docs_entry,
        }
    }
}

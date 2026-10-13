//! Small data types shared between the UI state machine and the backend workers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use everythux_core::config::PropertiesTarget;
use everythux_core::ids::EntryId;
use everythux_core::model::EntryKind;
use everythux_core::snapshot::IndexSnapshot;

/// What a "properties" request is about, resolved against the snapshot at click time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PropTarget {
    /// Absolute path (raw bytes preserved).
    pub(crate) path: PathBuf,
    /// File, folder, ...
    pub(crate) kind: EntryKind,
    /// Size in bytes; for folders the recursive total.
    pub(crate) size: u64,
    /// Modification time in ns since the Unix epoch.
    pub(crate) mtime_ns: i64,
}

impl PropTarget {
    /// The folder that "re-index" and "open folder" operate on: the target itself for folders,
    /// the containing folder otherwise.
    pub(crate) fn folder(&self) -> &Path {
        if self.kind.is_dir() {
            &self.path
        } else {
            self.path.parent().unwrap_or(&self.path)
        }
    }
}

/// Filesystem facts read from `symlink_metadata` on a worker thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FsMeta {
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    /// Full `st_mode` (type and permission bits).
    pub(crate) mode: u32,
    pub(crate) modified_ns: i64,
    pub(crate) accessed_ns: i64,
    pub(crate) changed_ns: i64,
    /// Birth time; not every filesystem or kernel reports it.
    pub(crate) created_ns: Option<i64>,
}

/// Everything the built-in Properties window shows besides the [`PropTarget`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PropertiesInfo {
    /// Files below the folder according to the index (0 for files).
    pub(crate) files: u64,
    /// Folders below the folder according to the index.
    pub(crate) folders: u64,
    /// Live filesystem facts, or why they could not be read.
    pub(crate) meta: Result<FsMeta, String>,
}

/// The current result list together with the snapshot its ids refer to.
///
/// Rows must always be resolved against [`Self::snapshot`], never against a newer snapshot.
#[derive(Debug, Clone)]
pub(crate) struct ResultsView {
    pub(crate) ids: Arc<[EntryId]>,
    pub(crate) snapshot: Arc<IndexSnapshot>,
    pub(crate) total_entries: usize,
    pub(crate) elapsed: std::time::Duration,
}

impl ResultsView {
    /// Number of rows.
    pub(crate) fn len(&self) -> usize {
        self.ids.len()
    }

    /// Entry shown in `row`.
    pub(crate) fn entry(&self, row: usize) -> Option<EntryId> {
        self.ids.get(row).copied()
    }

    /// Full path of the entry in `row`.
    pub(crate) fn full_path(&self, row: usize) -> Option<PathBuf> {
        self.entry(row).map(|id| self.snapshot.full_path(id))
    }

    /// Containing folder of the entry in `row`.
    pub(crate) fn folder_path(&self, row: usize) -> Option<PathBuf> {
        self.entry(row)
            .map(|id| self.snapshot.parent_path(id).to_path_buf())
    }

    /// Whether the entry in `row` is a folder.
    pub(crate) fn is_dir(&self, row: usize) -> bool {
        self.entry(row)
            .is_some_and(|id| self.snapshot.entries.kind[id.index()].is_dir())
    }

    /// Properties target for the entry in `row`: the containing folder (the folder itself for folder
    /// rows) when `mode` is `Parent`, else the item.
    pub(crate) fn prop_target(&self, row: usize, mode: PropertiesTarget) -> Option<PropTarget> {
        let id = self.entry(row)?;
        let snap = &*self.snapshot;
        let e = &snap.entries;
        let i = id.index();
        if mode == PropertiesTarget::Parent {
            // Folder rows: the folder itself. Other rows: the containing folder.
            let dir = if e.kind[i].is_dir() && !e.self_dir[i].is_none() {
                e.self_dir[i]
            } else {
                e.dir[i]
            };
            Some(PropTarget {
                path: snap.dirs.path(dir).to_path_buf(),
                kind: EntryKind::Dir,
                size: snap.dirs.total_size[dir.index()],
                mtime_ns: snap.dirs.mtime_ns[dir.index()],
            })
        } else {
            Some(PropTarget {
                path: snap.full_path(id),
                kind: e.kind[i],
                size: e.size[i],
                mtime_ns: e.mtime_ns[i],
            })
        }
    }
}

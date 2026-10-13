//! The in-memory, struct-of-arrays view of the whole index.
//!
//! `everythux-index` builds an [`IndexSnapshot`] from SQLite (`Store::load_snapshot`) and
//! `everythux-search` scans it. Columns are separate `Vec`s so a search over names touches only the
//! name arena; all strings live in contiguous [`ByteArena`]s instead of millions of `String`s.

use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::ids::{DirId, EntryId, VolumeId};
use crate::model::{EntryKind, VolumeInfo};

/// Many byte strings stored back to back: `bytes[offsets[i]..offsets[i + 1]]` is string `i`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteArena {
    bytes: Vec<u8>,
    /// `len() + 1` offsets; always starts with `0`.
    offsets: Vec<u32>,
}

impl Default for ByteArena {
    fn default() -> Self {
        Self::new()
    }
}

impl ByteArena {
    /// An empty arena.
    pub fn new() -> Self {
        Self {
            bytes: Vec::new(),
            offsets: vec![0],
        }
    }

    /// An empty arena with room for `strings` strings totalling `bytes` bytes.
    pub fn with_capacity(strings: usize, bytes: usize) -> Self {
        let mut offsets = Vec::with_capacity(strings + 1);
        offsets.push(0);
        Self {
            bytes: Vec::with_capacity(bytes),
            offsets,
        }
    }

    /// Appends a string and returns its index.
    ///
    /// # Panics
    /// If the arena would exceed `u32::MAX` bytes (4 GiB of names, far beyond realistic indexes).
    pub fn push(&mut self, s: &[u8]) -> u32 {
        let idx = self.len() as u32;
        self.bytes.extend_from_slice(s);
        let end = u32::try_from(self.bytes.len()).expect("ByteArena exceeds 4 GiB");
        self.offsets.push(end);
        idx
    }

    /// String `i`.
    #[inline]
    pub fn get(&self, i: usize) -> &[u8] {
        let start = self.offsets[i] as usize;
        let end = self.offsets[i + 1] as usize;
        &self.bytes[start..end]
    }

    /// Number of strings.
    #[inline]
    pub fn len(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// Whether the arena holds no strings.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The concatenated bytes of all strings (for bulk substring scans).
    #[inline]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The offsets array (`len() + 1` entries, starting at 0).
    #[inline]
    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }

    /// Total heap bytes used.
    pub fn heap_size(&self) -> usize {
        self.bytes.capacity() + self.offsets.capacity() * size_of::<u32>()
    }
}

/// All folders. `DirId(i)` indexes every column.
#[derive(Debug, Clone, Default)]
pub struct DirTable {
    /// Absolute path of each folder, raw bytes, no trailing slash (except `/` itself).
    pub paths: ByteArena,
    /// Parent folder, or [`DirId::NONE`] for a volume root.
    pub parent: Vec<DirId>,
    /// Owning volume.
    pub volume: Vec<VolumeId>,
    /// Recursive size of everything below the folder, in bytes.
    pub total_size: Vec<u64>,
    /// Folder mtime (ns).
    pub mtime_ns: Vec<i64>,
    /// The entry representing this folder itself in [`EntryTable`] ([`EntryId::NONE`] for volume
    /// roots, which have no containing folder in the index).
    pub entry: Vec<EntryId>,
}

impl DirTable {
    /// Number of folders.
    pub fn len(&self) -> usize {
        self.parent.len()
    }

    /// Whether there are no folders.
    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    /// Path of a folder as a [`Path`].
    pub fn path(&self, id: DirId) -> &Path {
        Path::new(std::ffi::OsStr::from_bytes(self.paths.get(id.index())))
    }
}

/// All entries (files, folders, symlinks, …). `EntryId(i)` indexes every column.
#[derive(Debug, Clone, Default)]
pub struct EntryTable {
    /// Raw filename bytes (exactly as on disk).
    pub names: ByteArena,
    /// [`crate::text::fold`]ed names, for case-insensitive search.
    pub names_fold: ByteArena,
    /// Containing folder.
    pub dir: Vec<DirId>,
    /// Kind of object.
    pub kind: Vec<EntryKind>,
    /// Size in bytes (for folders: [`DirTable::total_size`] of the folder).
    pub size: Vec<u64>,
    /// Modification time, ns since the Unix epoch.
    pub mtime_ns: Vec<i64>,
    /// Index into [`IndexSnapshot::extensions`]; `0` means "no extension".
    pub ext: Vec<u16>,
    /// For folders: the folder's own [`DirId`]; otherwise [`DirId::NONE`].
    pub self_dir: Vec<DirId>,
}

impl EntryTable {
    /// Number of entries.
    pub fn len(&self) -> usize {
        self.dir.len()
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.dir.is_empty()
    }
}

/// The whole index in memory. Immutable once built; replaced wholesale (behind an `Arc`) when the
/// index changes.
#[derive(Debug, Clone, Default)]
pub struct IndexSnapshot {
    /// Indexed volumes. `VolumeId(i)` indexes this.
    pub volumes: Vec<VolumeInfo>,
    /// All folders.
    pub dirs: DirTable,
    /// All entries.
    pub entries: EntryTable,
    /// Interned lowercase extensions. Index `0` is always the empty string ("no extension").
    pub extensions: Vec<Box<str>>,
}

impl IndexSnapshot {
    /// An empty snapshot (with the reserved "no extension" slot).
    pub fn empty() -> Self {
        Self {
            extensions: vec![Box::from("")],
            ..Self::default()
        }
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the snapshot is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Raw name of an entry.
    pub fn name(&self, id: EntryId) -> &[u8] {
        self.entries.names.get(id.index())
    }

    /// Path of the folder containing the entry (the "Path" column).
    pub fn parent_path(&self, id: EntryId) -> &Path {
        self.dirs.path(self.entries.dir[id.index()])
    }

    /// Full path of the entry.
    pub fn full_path(&self, id: EntryId) -> std::path::PathBuf {
        let name = std::ffi::OsStr::from_bytes(self.name(id));
        self.parent_path(id).join(name)
    }

    /// Lowercase extension of an entry (`""` if none).
    pub fn extension(&self, id: EntryId) -> &str {
        &self.extensions[self.entries.ext[id.index()] as usize]
    }

    /// Approximate heap usage in bytes (for the status bar / memory budget checks).
    pub fn heap_size(&self) -> usize {
        let e = &self.entries;
        let d = &self.dirs;
        e.names.heap_size()
            + e.names_fold.heap_size()
            + e.dir.capacity() * 4
            + e.kind.capacity()
            + e.size.capacity() * 8
            + e.mtime_ns.capacity() * 8
            + e.ext.capacity() * 2
            + e.self_dir.capacity() * 4
            + d.paths.heap_size()
            + d.parent.capacity() * 4
            + d.volume.capacity() * 4
            + d.total_size.capacity() * 8
            + d.mtime_ns.capacity() * 8
            + d.entry.capacity() * 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_arena_is_usable() {
        let mut a = ByteArena::default();
        a.push(b"x");
        assert_eq!(a.get(0), b"x");
        let mut snap = IndexSnapshot::empty();
        snap.entries.names.push(b"y");
        assert_eq!(snap.entries.names.get(0), b"y");
    }

    #[test]
    fn arena_roundtrip() {
        let mut a = ByteArena::new();
        assert!(a.is_empty());
        assert_eq!(a.push(b"foo"), 0);
        assert_eq!(a.push(b""), 1);
        assert_eq!(a.push(b"bar\xff"), 2);
        assert_eq!(a.len(), 3);
        assert_eq!(a.get(0), b"foo");
        assert_eq!(a.get(1), b"");
        assert_eq!(a.get(2), b"bar\xff");
        assert_eq!(a.bytes(), b"foobar\xff");
    }
}

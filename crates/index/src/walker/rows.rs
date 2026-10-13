//! Row types the walker hands to the store writer.
//!
//! A [`DirListing`] holds all entries of one folder. Names, folded names and extensions of all its
//! entries live in one byte arena (`text`), so listing a folder costs a handful of allocations
//! regardless of how many entries it contains.

use std::borrow::Cow;

use everythux_core::model::EntryKind;
use everythux_core::text;

/// A byte range inside [`DirListing::text`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Span {
    off: u32,
    len: u32,
}

impl Span {
    const fn range(self) -> std::ops::Range<usize> {
        self.off as usize..(self.off + self.len) as usize
    }
}

/// How the store has to treat a [`DirListing`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingKind {
    /// The folder is new: insert its row and entries.
    Insert,
    /// The folder is already stored: diff its stored entries against this listing.
    Diff,
    /// The folder is stored but its entries were deleted beforehand (subtree replace): update its
    /// row and insert the entries.
    Reset,
}

/// One directory entry, with its strings stored in the owning [`DirListing`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryRow {
    /// What the entry is.
    pub kind: EntryKind,
    /// Size in bytes (0 for folders; the store fills in recursive sizes).
    pub size: u64,
    /// Modification time, ns since the epoch.
    pub mtime_ns: i64,
    /// Inode number.
    pub inode: u64,
    /// Owner.
    pub uid: u32,
    /// Raw `st_mode` (type and permission bits).
    pub mode: u32,
    /// Id of the folder this entry stands for when the walker descends into it, `0` otherwise
    /// (files, symlinks, and folders on other filesystems such as mount points).
    pub child_dir: i64,
    pub(crate) name: Span,
    pub(crate) fold: Span,
    pub(crate) ext: Span,
}

/// All entries of one folder plus the folder's own metadata.
#[derive(Debug, Clone)]
pub struct DirListing {
    /// Database id of the folder (pre-assigned by the walker).
    pub id: i64,
    /// Id of the containing folder, `None` for a volume root.
    pub parent_id: Option<i64>,
    /// How the store must apply this listing.
    pub kind: ListingKind,
    /// Absolute path of the folder, raw bytes, no trailing slash (except `/`).
    pub path: Vec<u8>,
    /// Inode of the folder.
    pub inode: u64,
    /// Folder mtime (ns), read *before* listing so a concurrent change is seen as newer next time.
    pub mtime_ns: i64,
    /// The entries, in inode order.
    pub entries: Vec<EntryRow>,
    text: Vec<u8>,
}

impl DirListing {
    pub(crate) fn new(
        id: i64,
        parent_id: Option<i64>,
        kind: ListingKind,
        path: Vec<u8>,
        inode: u64,
        mtime_ns: i64,
        capacity: usize,
    ) -> Self {
        Self {
            id,
            parent_id,
            kind,
            path,
            inode,
            mtime_ns,
            entries: Vec::with_capacity(capacity),
            text: Vec::with_capacity(capacity * 24),
        }
    }

    /// Raw file name of an entry.
    pub fn name(&self, row: &EntryRow) -> &[u8] {
        &self.text[row.name.range()]
    }

    /// Case-folded name of an entry (valid UTF-8, see [`everythux_core::text::fold`]).
    pub fn fold(&self, row: &EntryRow) -> &[u8] {
        &self.text[row.fold.range()]
    }

    /// Lowercase extension of an entry, empty if it has none.
    pub fn ext(&self, row: &EntryRow) -> &[u8] {
        &self.text[row.ext.range()]
    }

    fn push_span(&mut self, bytes: &[u8]) -> Span {
        let off = u32::try_from(self.text.len()).unwrap_or(u32::MAX);
        self.text.extend_from_slice(bytes);
        Span {
            off,
            len: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
        }
    }

    /// Appends an entry; derives the folded name and extension from `name`.
    pub(crate) fn push(&mut self, name: &[u8], mut row: EntryRow) {
        let name_span = self.push_span(name);
        row.name = name_span;
        row.fold = if name.iter().all(|b| b.is_ascii() && !b.is_ascii_uppercase()) {
            name_span
        } else if name.is_ascii() {
            let start = self.text.len();
            self.text.extend(name.iter().map(u8::to_ascii_lowercase));
            self.span_from(start)
        } else {
            let folded = text::fold(name);
            self.push_span(folded.as_bytes())
        };
        row.ext = match text::extension(name) {
            None => Span::default(),
            // A borrowed extension is a suffix of `name`: point into the stored name.
            Some(Cow::Borrowed(ext)) => Span {
                off: name_span.off + (name_span.len - u32::try_from(ext.len()).unwrap_or(0)),
                len: u32::try_from(ext.len()).unwrap_or(0),
            },
            Some(Cow::Owned(ext)) => self.push_span(ext.as_bytes()),
        };
        self.entries.push(row);
    }

    fn span_from(&self, start: usize) -> Span {
        Span {
            off: u32::try_from(start).unwrap_or(u32::MAX),
            len: u32::try_from(self.text.len() - start).unwrap_or(u32::MAX),
        }
    }
}

impl EntryRow {
    /// A row with metadata only; the walker fills the string spans through [`DirListing::push`].
    pub(crate) const fn new(
        kind: EntryKind,
        size: u64,
        mtime_ns: i64,
        inode: u64,
        uid: u32,
        mode: u32,
    ) -> Self {
        Self {
            kind,
            size,
            mtime_ns,
            inode,
            uid,
            mode,
            child_dir: 0,
            name: Span { off: 0, len: 0 },
            fold: Span { off: 0, len: 0 },
            ext: Span { off: 0, len: 0 },
        }
    }
}

/// Rows travelling from the walker to the writer thread (about [`crate::walker::WalkOptions::batch_rows`]
/// rows per batch).
#[derive(Debug, Default)]
pub struct ScanBatch {
    /// Listed folders.
    pub listings: Vec<DirListing>,
    /// Ids of known folders that no longer exist on disk.
    pub vanished: Vec<i64>,
    pub(crate) rows: usize,
}

impl ScanBatch {
    /// Number of entries plus folders in the batch (what the batch size limit counts).
    pub const fn rows(&self) -> usize {
        self.rows
    }

    /// Whether the batch carries nothing.
    pub fn is_empty(&self) -> bool {
        self.listings.is_empty() && self.vanished.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> EntryRow {
        EntryRow::new(EntryKind::File, 1, 2, 3, 4, 5)
    }

    #[test]
    fn plain_lowercase_names_share_storage() {
        let mut l = DirListing::new(1, None, ListingKind::Insert, b"/".to_vec(), 1, 0, 4);
        l.push(b"readme.md", row());
        let r = l.entries[0];
        assert_eq!(l.name(&r), b"readme.md");
        assert_eq!(l.fold(&r), b"readme.md");
        assert_eq!(l.ext(&r), b"md");
        assert_eq!(
            l.text.len(),
            "readme.md".len(),
            "name, fold and ext are one allocation"
        );
    }

    #[test]
    fn folds_match_core_for_all_name_shapes() {
        let mut l = DirListing::new(1, None, ListingKind::Insert, b"/".to_vec(), 1, 0, 8);
        let names: [&[u8]; 6] = [
            b"ReadMe.MD",
            "ÄPFEL.TXT".as_bytes(),
            b"bad\xffName.Z",
            b".bashrc",
            b"noext",
            b"x.",
        ];
        for n in names {
            l.push(n, row());
        }
        for (n, r) in names.iter().zip(&l.entries) {
            assert_eq!(l.name(r), *n);
            assert_eq!(l.fold(r), text::fold(n).as_bytes(), "{n:?}");
            let want = text::extension(n).map(Cow::into_owned).unwrap_or_default();
            assert_eq!(l.ext(r), want.as_bytes(), "{n:?}");
        }
    }
}

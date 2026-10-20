//! Folders the index may be out of sync with.
//!
//! With automatic indexing off, the app records every folder its own file operations touched
//! (delete, move, rename, paste) here instead of rescanning them. `Ctrl+Shift+R` later hands the
//! set to the indexer as one refresh. The set is a deduplicated, sorted collection of absolute
//! folder paths, kept as raw bytes: Linux file names need not be UTF-8.
//!
//! It is persisted in a small file next to the config (`dirty-folders.list`: absolute paths
//! separated by NUL bytes, which cannot occur in a path), so it survives restarts.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

/// A deduplicated set of absolute folder paths that need re-listing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DirtyFolders {
    folders: BTreeSet<PathBuf>,
}

impl DirtyFolders {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of folders.
    #[must_use]
    pub fn len(&self) -> usize {
        self.folders.len()
    }

    /// Whether no folder is recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty()
    }

    /// Whether `folder` is recorded.
    #[must_use]
    pub fn contains(&self, folder: &Path) -> bool {
        self.folders.contains(folder)
    }

    /// The folders in path order.
    pub fn iter(&self) -> impl Iterator<Item = &Path> {
        self.folders.iter().map(PathBuf::as_path)
    }

    /// The folders in path order, owned (the payload of an indexer refresh job).
    #[must_use]
    pub fn to_vec(&self) -> Vec<PathBuf> {
        self.folders.iter().cloned().collect()
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        self.folders.clear();
    }

    /// Records `folder`. Relative paths are ignored (they cannot be located later). Returns
    /// whether the set changed.
    pub fn add_folder(&mut self, folder: &Path) -> bool {
        folder.is_absolute() && self.folders.insert(folder.to_path_buf())
    }

    /// Records the folder that contains `path`. The root has none. Returns whether the set changed.
    pub fn add_parent_of(&mut self, path: &Path) -> bool {
        path.parent().is_some_and(|parent| self.add_folder(parent))
    }

    /// `path` was deleted or moved to the trash: its parent lost an entry.
    pub fn note_removed(&mut self, path: &Path) -> bool {
        self.add_parent_of(path)
    }

    /// `path` was created by a copy: its parent gained an entry.
    pub fn note_added(&mut self, path: &Path) -> bool {
        self.add_parent_of(path)
    }

    /// `from` was renamed or moved to `to`: both parents changed, and when the item is a folder
    /// (`to_is_dir`) its new location is recorded too, since its subtree is new there.
    pub fn note_moved(&mut self, from: &Path, to: &Path, to_is_dir: bool) -> bool {
        let mut changed = self.add_parent_of(from);
        changed |= self.add_parent_of(to);
        if to_is_dir {
            changed |= self.add_folder(to);
        }
        changed
    }

    /// Parses the file format (see the module docs); empty and relative entries are dropped.
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Self {
        let folders = bytes
            .split(|&b| b == 0)
            .filter(|part| !part.is_empty())
            .map(|part| PathBuf::from(OsStr::from_bytes(part)))
            .filter(|path| path.is_absolute())
            .collect();
        Self { folders }
    }

    /// The file format: every path followed by a NUL byte.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for folder in &self.folders {
            out.extend_from_slice(folder.as_os_str().as_bytes());
            out.push(0);
        }
        out
    }

    /// Loads `path`. A missing file is an empty set.
    ///
    /// # Errors
    ///
    /// The I/O error as text; the caller should carry on with an empty set.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Self::parse(&bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Writes the set atomically: temp file next to it, `fsync`, rename. An empty set removes
    /// the file.
    ///
    /// # Errors
    ///
    /// The I/O error as text.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let io = |e: std::io::Error| format!("{}: {e}", path.display());
        if self.is_empty() {
            return match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(io(e)),
            };
        }
        let dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).map_err(io)?;
        let mut name = std::ffi::OsString::from(".");
        name.push(path.file_name().unwrap_or(OsStr::new("dirty-folders.list")));
        name.push(format!(".{}.tmp", std::process::id()));
        let tmp = dir.join(name);
        let mut file = std::fs::File::create(&tmp).map_err(io)?;
        file.write_all(&self.to_bytes()).map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            io(e)
        })
    }
}

impl FromIterator<PathBuf> for DirtyFolders {
    fn from_iter<T: IntoIterator<Item = PathBuf>>(iter: T) -> Self {
        let mut set = Self::new();
        for folder in iter {
            set.add_folder(&folder);
        }
        set
    }
}

impl From<DirtyFolders> for Vec<PathBuf> {
    fn from(set: DirtyFolders) -> Self {
        set.folders.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_from_bytes(bytes: &[u8]) -> PathBuf {
        PathBuf::from(OsStr::from_bytes(bytes))
    }

    fn p(s: &str) -> &Path {
        Path::new(s)
    }

    #[test]
    fn folders_are_deduplicated_and_sorted() {
        let mut set = DirtyFolders::new();
        assert!(set.add_folder(p("/b")));
        assert!(set.add_folder(p("/a/x")));
        assert!(!set.add_folder(p("/b")), "already there");
        assert!(set.note_removed(p("/a/gone.txt")));
        assert!(!set.note_removed(p("/a/other.txt")), "same parent");
        assert_eq!(set.to_vec(), [PathBuf::from("/a"), "/a/x".into(), "/b".into()]);
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn relative_paths_and_the_root_have_no_folder() {
        let mut set = DirtyFolders::new();
        assert!(!set.add_folder(p("relative/dir")));
        assert!(!set.add_parent_of(p("/")), "the root has no parent");
        assert!(
            !set.add_parent_of(p("file.txt")),
            "parent of a bare name is empty"
        );
        assert!(set.is_empty());
        assert!(set.add_parent_of(p("/top.txt")));
        assert!(set.contains(p("/")), "a file in the root dirties the root");
    }

    #[test]
    fn a_move_across_folders_dirties_both_parents() {
        let mut set = DirtyFolders::new();
        assert!(set.note_moved(p("/home/a/f.txt"), p("/home/b/f.txt"), false));
        assert_eq!(set.to_vec(), [PathBuf::from("/home/a"), "/home/b".into()]);
    }

    #[test]
    fn a_moved_or_renamed_folder_is_recorded_itself_at_its_new_place() {
        let mut set = DirtyFolders::new();
        set.note_moved(p("/home/a/proj"), p("/home/a/project"), true);
        assert_eq!(set.to_vec(), [PathBuf::from("/home/a"), "/home/a/project".into()]);
        assert!(!set.contains(p("/home/a/proj")), "the old name no longer exists");
        // Same-folder renames of files add the one parent only.
        let mut files = DirtyFolders::new();
        files.note_moved(p("/x/a.txt"), p("/x/b.txt"), false);
        assert_eq!(files.to_vec(), [PathBuf::from("/x")]);
    }

    #[test]
    fn a_copy_dirties_only_the_destination_folder() {
        let mut set = DirtyFolders::new();
        set.note_added(p("/dest/report (copy).pdf"));
        assert_eq!(set.to_vec(), [PathBuf::from("/dest")]);
    }

    #[test]
    fn non_utf8_paths_are_kept_byte_for_byte() {
        let bad_dir = path_from_bytes(b"/data/bad\xffdir");
        let file = bad_dir.join(path_from_bytes(b"f\xfe.txt"));
        let mut set = DirtyFolders::new();
        assert!(set.note_removed(&file));
        assert!(
            !set.note_removed(&bad_dir.join("other")),
            "same invalid parent dedups"
        );
        assert!(set.contains(&bad_dir));
        let again = DirtyFolders::parse(&set.to_bytes());
        assert_eq!(again, set, "the file format keeps raw bytes");
        assert_eq!(again.to_vec(), [bad_dir]);
    }

    #[test]
    fn parsing_drops_empty_and_relative_entries() {
        let set = DirtyFolders::parse(b"/a\0\0rel/b\0/c\0/a\0");
        assert_eq!(set.to_vec(), [PathBuf::from("/a"), "/c".into()]);
        assert!(DirtyFolders::parse(b"").is_empty());
    }

    #[test]
    fn save_and_load_round_trip_and_an_empty_set_removes_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("sub/dirty-folders.list");
        assert_eq!(DirtyFolders::load(&file), Ok(DirtyFolders::new()), "missing file");

        let mut set = DirtyFolders::new();
        set.add_folder(p("/a/b"));
        set.add_folder(&path_from_bytes(b"/x/\xff"));
        set.save(&file).expect("save");
        assert_eq!(DirtyFolders::load(&file), Ok(set));
        let leftovers: Vec<_> = std::fs::read_dir(file.parent().expect("parent"))
            .expect("dir")
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        assert_eq!(leftovers.len(), 1, "no temp file left behind: {leftovers:?}");

        DirtyFolders::new().save(&file).expect("save empty");
        assert!(!file.exists());
        DirtyFolders::new()
            .save(&file)
            .expect("removing a missing file is fine");
    }
}

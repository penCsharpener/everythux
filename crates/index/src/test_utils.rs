//! Synthetic directory trees for tests and benchmarks.
//!
//! [`TreeBuilder`] creates hand-written trees (including awkward names and symlinks);
//! [`generate_tree`] creates large, reproducible ones for benchmarks (same seed, same tree).

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// A path whose final bytes are `bytes`, for names that are not valid UTF-8
/// (`raw_name(b"bad\xffname")`).
pub fn raw_name(bytes: &[u8]) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(bytes))
}

/// Builds a tree of folders, files, symlinks under a root. Every method acts immediately and
/// returns `&Self`, so calls chain with `?`:
///
/// ```
/// # use everythux_index::test_utils::TreeBuilder;
/// # fn main() -> std::io::Result<()> {
/// let tmp = tempfile::tempdir()?;
/// TreeBuilder::new(tmp.path())
///     .dir("a/b")?
///     .file("a/b/c.txt", 10)?
///     .symlink("loop", ".")?;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone)]
pub struct TreeBuilder {
    root: PathBuf,
}

impl TreeBuilder {
    /// A builder rooted at `root` (which must exist).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The root folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Absolute path of `rel`.
    pub fn path(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.root.join(rel)
    }

    /// Creates a folder (and its parents).
    pub fn dir(&self, rel: impl AsRef<Path>) -> io::Result<&Self> {
        fs::create_dir_all(self.path(rel))?;
        Ok(self)
    }

    /// Creates a file of `size` bytes (sparse: no data is written).
    pub fn file(&self, rel: impl AsRef<Path>, size: u64) -> io::Result<&Self> {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        File::create(path)?.set_len(size)?;
        Ok(self)
    }

    /// Creates a file with the given contents.
    pub fn file_with(&self, rel: impl AsRef<Path>, contents: &[u8]) -> io::Result<&Self> {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, contents)?;
        Ok(self)
    }

    /// Creates a symlink `rel` pointing at `target` (which need not exist).
    pub fn symlink(&self, rel: impl AsRef<Path>, target: impl AsRef<Path>) -> io::Result<&Self> {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        std::os::unix::fs::symlink(target, path)?;
        Ok(self)
    }

    /// Removes a file, symlink or folder tree.
    pub fn remove(&self, rel: impl AsRef<Path>) -> io::Result<&Self> {
        let path = self.path(rel);
        if fs::symlink_metadata(&path)?.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
        Ok(self)
    }

    /// Sets the mtime of `rel` to `secs` seconds since the epoch (files and folders).
    pub fn set_mtime(&self, rel: impl AsRef<Path>, secs: u64) -> io::Result<&Self> {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        File::open(self.path(rel))?.set_modified(t)?;
        Ok(self)
    }
}

/// What [`generate_tree`] created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeStats {
    /// Folders created, excluding the root.
    pub dirs: usize,
    /// Files created.
    pub files: usize,
    /// Sum of all file sizes.
    pub bytes: u64,
}

/// `SplitMix64`: tiny, fast, good enough for reproducible test data.
#[derive(Debug, Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % (n.max(1) as u64)).unwrap_or(0)
    }
}

const EXTENSIONS: [&str; 12] = [
    "txt", "rs", "md", "jpg", "png", "pdf", "mp3", "log", "toml", "json", "c", "h",
];
const WORDS: [&str; 16] = [
    "report", "photo", "notes", "build", "data", "image", "backup", "draft", "config", "index", "main",
    "test", "archive", "music", "video", "src",
];

/// Creates `dirs` folders and `files` files below `root` (which must exist and be empty).
///
/// The shape is deterministic for a given `seed`: folders hang off random earlier folders
/// (bushy at the top, deep in places), files are spread unevenly, names mix words, numbers,
/// mixed case and a few non-ASCII characters, and sizes are log-uniform up to ~1 MiB (sparse).
pub fn generate_tree(root: &Path, files: usize, dirs: usize, seed: u64) -> io::Result<TreeStats> {
    let mut rng = Rng(seed);
    let mut folders: Vec<PathBuf> = vec![root.to_path_buf()];
    for i in 0..dirs {
        // Prefer recent folders half of the time: that makes some chains deep.
        let parent_idx = if rng.next() & 1 == 0 {
            rng.below(folders.len())
        } else {
            folders.len() - 1 - rng.below(folders.len().min(16))
        };
        let name = format!("{}_{i}", WORDS[rng.below(WORDS.len())]);
        let path = folders[parent_idx].join(name);
        fs::create_dir(&path)?;
        folders.push(path);
    }

    let mut stats = TreeStats {
        dirs,
        files,
        bytes: 0,
    };
    for i in 0..files {
        // Squaring the random index skews files towards the first folders.
        let skew = rng.below(folders.len());
        let folder = &folders[rng.below(skew + 1)];
        let ext = EXTENSIONS[rng.below(EXTENSIONS.len())];
        let word = WORDS[rng.below(WORDS.len())];
        let name = match rng.below(10) {
            0 => format!("{}_{i}.{ext}", word.to_uppercase()),
            1 => format!("Ünï-{word}-{i}.{ext}"),
            _ => format!("{word}_{i}.{ext}"),
        };
        let size = if rng.below(8) == 0 {
            0
        } else {
            1_u64 << rng.below(21)
        };
        File::create(folder.join(name))?.set_len(size)?;
        stats.bytes += size;
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_creates_everything_requested() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let tb = TreeBuilder::new(tmp.path());
        tb.dir("a/b")
            .and_then(|t| t.file("a/b/c.txt", 10))
            .and_then(|t| t.file(raw_name(b"bad\xffname"), 1))
            .and_then(|t| t.symlink("loop", "."))
            .expect("build");
        assert_eq!(fs::metadata(tb.path("a/b/c.txt")).expect("stat").len(), 10);
        assert!(fs::symlink_metadata(tb.path("loop")).expect("lstat").is_symlink());
        assert!(tb.path(raw_name(b"bad\xffname")).exists());
        tb.remove("a").expect("remove");
        assert!(!tb.path("a").exists());
    }

    fn walk(root: &Path, dir: &Path) -> Vec<(PathBuf, u64)> {
        let mut out = Vec::new();
        for e in fs::read_dir(dir).expect("read_dir").flatten() {
            let meta = e.metadata().expect("meta");
            let rel = e.path().strip_prefix(root).expect("prefix").to_path_buf();
            if meta.is_dir() {
                out.extend(walk(root, &e.path()));
            }
            out.push((rel, if meta.is_dir() { 0 } else { meta.len() }));
        }
        out
    }

    #[test]
    fn generator_is_deterministic() {
        let listing = |seed| {
            let tmp = tempfile::tempdir().expect("tempdir");
            let stats = generate_tree(tmp.path(), 300, 40, seed).expect("generate");
            let mut names: Vec<_> = walk(tmp.path(), tmp.path());
            names.sort();
            (stats, names)
        };
        let a = listing(7);
        assert_eq!(a, listing(7));
        assert_ne!(a.1, listing(8).1);
        assert_eq!(a.0.files, 300);
        assert_eq!(a.1.len(), 340);
    }
}

//! Parallel, exclusion-pruning, one-filesystem directory walker (ROADMAP Phase 1).
//!
//! One walk covers one volume root (or a subtree of it). Folders are processed as independent
//! tasks on a `rayon` pool: a task opens its folder, reads the names with `getdents64` (no
//! per-entry allocation), skips excluded entries *before* touching them, `statx`-style stats the
//! rest in inode order, emits a [`DirListing`] and spawns one task per sub-folder. Tasks open
//! folders by absolute path, so no directory file descriptor outlives its task and the number of
//! open descriptors is bounded by the number of worker threads.
//!
//! The walker
//! - never follows symlinks (they are recorded as [`EntryKind::Symlink`]),
//! - never crosses `st_dev` when [`WalkOptions::one_filesystem`] is set: the mount-point folder
//!   itself is recorded, but not read,
//! - counts unreadable folders and failed stats instead of failing,
//! - checks a cancellation flag per folder and per few thousand entries,
//! - batches rows (about [`WalkOptions::batch_rows`]) into a **bounded** channel.
//!
//! `follow_symlinks` from the config is not supported yet; loops cannot occur because links are
//! never followed.
//!
//! # Incremental walks
//!
//! With [`WalkMode::Quick`] the walker is given the folders the database already knows
//! ([`KnownTree`]). A known folder whose mtime is unchanged is not read at all (its known
//! sub-folders are still visited); a changed one is re-listed and flagged [`ListingKind::Diff`] so
//! the store diffs it. New folders get fresh ids from the shared [`IdAlloc`].

mod known;
mod rows;

use std::cell::RefCell;
use std::ffi::{CStr, OsStr};
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crossbeam_channel::{Sender, bounded};
use everythux_core::exclusion::ExclusionSet;
use everythux_core::model::EntryKind;
use rustix::fs::{AtFlags, CWD, FileType, Mode, OFlags, RawDir, Stat, openat, statat};
use rustix::io::Errno;

pub use known::{KnownDir, KnownRow, KnownTree};
pub use rows::{DirListing, EntryRow, ListingKind, ScanBatch};

use crate::IndexError;

/// Entries between two cancellation checks inside one huge folder.
const CANCEL_CHECK_EVERY: usize = 4096;
/// Size of the per-thread `getdents64` buffer.
const DIRENT_BUF: usize = 64 * 1024;

/// Hands out database ids for new folders. Shared by all walks of one job.
#[derive(Debug)]
pub struct IdAlloc(AtomicI64);

impl IdAlloc {
    /// An allocator whose first id is `first`.
    pub const fn new(first: i64) -> Self {
        Self(AtomicI64::new(first))
    }

    /// The next unused id.
    pub fn next(&self) -> i64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

/// Static settings of a walk.
#[derive(Debug, Clone)]
pub struct WalkOptions {
    /// Entries matching these rules are not recorded (folders are not entered).
    pub exclusions: ExclusionSet,
    /// Do not descend into folders on other filesystems.
    pub one_filesystem: bool,
    /// Approximate rows per [`ScanBatch`].
    pub batch_rows: usize,
}

impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            exclusions: ExclusionSet::empty(),
            one_filesystem: true,
            batch_rows: 10_000,
        }
    }
}

/// The folder a replace-subtree walk starts from, which already exists in the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootRef {
    /// Id of the existing folder row.
    pub id: i64,
    /// Id of its parent folder, if any.
    pub parent_id: Option<i64>,
}

/// How much the database already knows.
#[derive(Debug, Clone)]
pub enum WalkMode {
    /// Everything below the root is new. With `existing_root` the root folder keeps its id and is
    /// emitted as [`ListingKind::Reset`] (the caller has deleted what was below it).
    Fresh {
        /// The root, if it already has a row.
        existing_root: Option<RootRef>,
    },
    /// Re-list only known folders whose mtime changed; list unknown folders fully.
    Quick {
        /// What the database knows.
        known: Arc<KnownTree>,
    },
    /// List the root only; known sub-folders are not entered, unknown ones are listed fully.
    Shallow {
        /// What the database knows (at least the root and its direct sub-folders).
        known: Arc<KnownTree>,
    },
}

/// Live counters of a walk, readable from any thread.
#[derive(Debug, Default)]
pub struct WalkCounters {
    /// Folders visited (listed or skipped as unchanged).
    pub dirs: AtomicU64,
    /// Folders actually read and listed.
    pub listed: AtomicU64,
    /// Entries recorded.
    pub entries: AtomicU64,
    /// Unreadable folders and failed stats.
    pub errors: AtomicU64,
}

/// Plain copy of [`WalkCounters`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WalkSummary {
    /// Folders visited (listed or skipped as unchanged).
    pub dirs: u64,
    /// Folders actually read and listed.
    pub listed: u64,
    /// Entries recorded.
    pub entries: u64,
    /// Unreadable folders and failed stats.
    pub errors: u64,
}

impl WalkCounters {
    /// Reads all counters.
    pub fn summary(&self) -> WalkSummary {
        WalkSummary {
            dirs: self.dirs.load(Ordering::Relaxed),
            listed: self.listed.load(Ordering::Relaxed),
            entries: self.entries.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
        }
    }
}

/// Cancellation flag plus counters, shared between the walk and whoever reports progress.
#[derive(Debug, Clone, Default)]
pub struct WalkControl {
    /// Set to stop the walk; workers notice at the next folder.
    pub cancel: Arc<AtomicBool>,
    /// Progress counters.
    pub counters: Arc<WalkCounters>,
}

impl WalkControl {
    /// A fresh control with its own flag and counters.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Builds the worker pool for walks. `threads == 1` means "walk on the calling thread" (`None`);
/// `0` means one thread per CPU.
pub fn make_pool(threads: usize) -> Option<rayon::ThreadPool> {
    let n = if threads == 0 {
        std::thread::available_parallelism().map_or(4, std::num::NonZero::get)
    } else {
        threads
    };
    if n <= 1 {
        return None;
    }
    rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .thread_name(|i| format!("everythux-walk-{i}"))
        .build()
        .inspect_err(|e| tracing::warn!(error = %e, "cannot build walker pool, walking single-threaded"))
        .ok()
}

/// One walk: where to start, what is known, and where to send rows.
#[derive(Debug)]
pub struct WalkJob<'a> {
    /// Absolute path of the folder to walk.
    pub root: &'a Path,
    /// What the database knows.
    pub mode: &'a WalkMode,
    /// Static settings.
    pub opts: &'a WalkOptions,
    /// Id source for new folders.
    pub ids: &'a IdAlloc,
    /// Cancellation and counters.
    pub ctl: &'a WalkControl,
    /// Worker pool; `None` walks on the calling thread.
    pub pool: Option<&'a rayon::ThreadPool>,
}

impl WalkJob<'_> {
    /// Runs the walk, sending batches into `sink`. Returns when all folders are done, the walk is
    /// cancelled, or the receiving side hung up. Blocks while the channel is full.
    ///
    /// # Errors
    /// If the root itself cannot be stat'ed or isn't a folder. Everything below the root fails
    /// softly (counted in [`WalkCounters::errors`]).
    pub fn run(&self, sink: &Sender<ScanBatch>) -> Result<(), IndexError> {
        let root_bytes = self.root.as_os_str().as_bytes();
        let stat = rustix::fs::lstat(root_bytes).map_err(|e| IndexError::errno(self.root, e))?;
        if !FileType::from_raw_mode(stat.st_mode).is_dir() {
            return Err(IndexError::io(
                self.root,
                std::io::Error::from(std::io::ErrorKind::NotADirectory),
            ));
        }
        let meta = DirMeta::from_stat(&stat);
        let slots = self.pool.map_or(1, rayon::ThreadPool::current_num_threads);
        let walker = Walker {
            job: self,
            sink,
            root_dev: meta.dev,
            has_exclusions: !self.opts.exclusions.is_empty(),
            slots: (0..slots).map(|_| Mutex::new(ScanBatch::default())).collect(),
        };

        let root_task = walker.root_task(root_bytes, meta);
        if let Some(pool) = self.pool {
            pool.install(|| rayon::scope(|s| walker.run_task(root_task, s)));
        } else {
            let mut stack = vec![root_task];
            while let Some(task) = stack.pop() {
                walker.process(task, &mut |child| stack.push(child));
            }
        }
        walker.flush_all();
        Ok(())
    }
}

/// Walks `root` without a database and returns the counters; the `everythux-cli scan --dry-run`
/// building block and the benchmark body.
///
/// # Errors
/// As [`WalkJob::run`].
pub fn dry_run(
    root: &Path,
    opts: &WalkOptions,
    ctl: &WalkControl,
    threads: usize,
) -> Result<WalkSummary, IndexError> {
    let pool = make_pool(threads);
    let ids = IdAlloc::new(1);
    let mode = WalkMode::Fresh { existing_root: None };
    let (tx, rx) = bounded::<ScanBatch>(4);
    let job = WalkJob {
        root,
        mode: &mode,
        opts,
        ids: &ids,
        ctl,
        pool: pool.as_ref(),
    };
    let result = std::thread::scope(|s| {
        s.spawn(|| for _batch in &rx {});
        let r = job.run(&tx);
        drop(tx);
        r
    });
    result.map(|()| ctl.counters.summary())
}

/// What a task knows about its folder from a previous stat.
#[derive(Debug, Clone, Copy)]
struct DirMeta {
    dev: u64,
    ino: u64,
    mtime_ns: i64,
}

impl DirMeta {
    fn from_stat(st: &Stat) -> Self {
        Self {
            dev: st.st_dev,
            ino: st.st_ino,
            mtime_ns: mtime_ns(st),
        }
    }
}

fn to_i64<T: Into<i128>>(v: T) -> i64 {
    let v: i128 = v.into();
    // Pre-1970 timestamps arrive as wrapped unsigned values on some targets.
    let v = if v >= 1 << 63 { v - (1 << 64) } else { v };
    i64::try_from(v).unwrap_or_default()
}

fn mtime_ns(st: &Stat) -> i64 {
    to_i64(st.st_mtime)
        .saturating_mul(1_000_000_000)
        .saturating_add(to_i64(st.st_mtime_nsec))
}

/// A folder waiting to be processed.
struct Task<'a> {
    path: Vec<u8>,
    id: i64,
    parent_id: Option<i64>,
    kind: ListingKind,
    meta: Option<DirMeta>,
    known: Option<&'a KnownDir>,
}

/// Per-thread reusable buffers.
struct Scratch {
    dirents: Vec<MaybeUninit<u8>>,
    /// NUL-terminated names of the folder being listed, back to back.
    names: Vec<u8>,
    raw: Vec<RawEnt>,
    /// `<folder>/<name>` of the entry being examined.
    path: Vec<u8>,
}

struct RawEnt {
    ino: u64,
    name_off: u32,
    name_len: u16,
    file_type: FileType,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch {
        dirents: vec![MaybeUninit::uninit(); DIRENT_BUF],
        names: Vec::new(),
        raw: Vec::new(),
        path: Vec::new(),
    });
}

struct Walker<'a, 'j> {
    job: &'a WalkJob<'j>,
    sink: &'a Sender<ScanBatch>,
    root_dev: u64,
    has_exclusions: bool,
    /// One batch per worker thread; a slot is only ever locked by its own thread (and by the
    /// final flush), so the mutex is uncontended.
    slots: Vec<Mutex<ScanBatch>>,
}

fn show(path: &[u8]) -> std::path::Display<'_> {
    Path::new(OsStr::from_bytes(path)).display()
}

impl<'a> Walker<'a, '_> {
    fn root_task(&self, root: &[u8], meta: DirMeta) -> Task<'a> {
        let known = match self.job.mode {
            WalkMode::Quick { known } | WalkMode::Shallow { known } => known.get(root),
            WalkMode::Fresh { .. } => None,
        };
        let (id, parent_id, kind) = match (self.job.mode, known) {
            (_, Some(k)) => (k.id, k.parent, ListingKind::Diff),
            (
                WalkMode::Fresh {
                    existing_root: Some(r),
                },
                None,
            ) => (r.id, r.parent_id, ListingKind::Reset),
            _ => (self.job.ids.next(), None, ListingKind::Insert),
        };
        Task {
            path: root.to_vec(),
            id,
            parent_id,
            kind,
            meta: Some(meta),
            known,
        }
    }

    fn cancelled(&self) -> bool {
        self.job.ctl.cancel.load(Ordering::Relaxed)
    }

    fn run_task<'s>(&'s self, task: Task<'s>, scope: &rayon::Scope<'s>) {
        self.process(task, &mut |child| scope.spawn(move |s| self.run_task(child, s)));
    }

    fn count_error(&self, path: &[u8], error: Errno) {
        self.job.ctl.counters.errors.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(path = %show(path), %error, "cannot read");
    }

    fn slot(&self) -> &Mutex<ScanBatch> {
        let idx = rayon::current_thread_index().unwrap_or(0);
        &self.slots[idx % self.slots.len()]
    }

    fn send(&self, batch: ScanBatch) {
        if !batch.is_empty() && self.sink.send(batch).is_err() {
            // The consumer is gone: stop all workers.
            self.job.ctl.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn emit(&self, listing: DirListing) {
        let full = {
            let mut batch = self.slot().lock().unwrap_or_else(PoisonError::into_inner);
            batch.rows += listing.entries.len() + 1;
            batch.listings.push(listing);
            (batch.rows >= self.job.opts.batch_rows).then(|| std::mem::take(&mut *batch))
        };
        if let Some(batch) = full {
            self.send(batch);
        }
    }

    fn vanished(&self, id: i64) {
        let mut batch = self.slot().lock().unwrap_or_else(PoisonError::into_inner);
        batch.vanished.push(id);
    }

    fn flush_all(&self) {
        for slot in &self.slots {
            let batch = std::mem::take(&mut *slot.lock().unwrap_or_else(PoisonError::into_inner));
            self.send(batch);
        }
    }

    /// Processes one folder; sub-folder tasks are handed to `push`.
    fn process<'s>(&'s self, task: Task<'s>, push: &mut dyn FnMut(Task<'s>)) {
        if self.cancelled() {
            return;
        }
        self.job.ctl.counters.dirs.fetch_add(1, Ordering::Relaxed);

        let meta = match task.meta {
            Some(meta) => meta,
            None => match rustix::fs::lstat(task.path.as_slice()) {
                Ok(st) if FileType::from_raw_mode(st.st_mode).is_dir() => DirMeta::from_stat(&st),
                Ok(_) | Err(Errno::NOENT | Errno::NOTDIR) => {
                    self.forget(&task);
                    return;
                }
                Err(e) => {
                    self.count_error(&task.path, e);
                    return;
                }
            },
        };
        if self.job.opts.one_filesystem && meta.dev != self.root_dev {
            self.forget(&task);
            return;
        }

        if let (WalkMode::Quick { known }, Some(k), ListingKind::Diff) =
            (self.job.mode, task.known, task.kind)
            && k.mtime_ns == meta.mtime_ns
        {
            // Unchanged: nothing to read, but known sub-folders may have changed.
            for name in &k.children {
                let mut path = task.path.clone();
                if path.last() != Some(&b'/') {
                    path.push(b'/');
                }
                path.extend_from_slice(name);
                if let Some(child) = known.get(&path) {
                    push(Task {
                        path,
                        id: child.id,
                        parent_id: Some(k.id),
                        kind: ListingKind::Diff,
                        meta: None,
                        known: Some(child),
                    });
                }
            }
            return;
        }

        let mut children = Vec::new();
        self.list(task, meta, &mut children);
        for child in children {
            push(child);
        }
    }

    /// A folder that was expected but is gone (or not a folder / on another filesystem anymore).
    fn forget(&self, task: &Task<'_>) {
        if task.known.is_some() {
            self.vanished(task.id);
        }
    }

    fn list<'s>(&'s self, task: Task<'s>, meta: DirMeta, children: &mut Vec<Task<'s>>) {
        let fd = match openat(
            CWD,
            task.path.as_slice(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(Errno::NOENT | Errno::NOTDIR) => {
                self.forget(&task);
                return;
            }
            Err(e) => {
                self.count_error(&task.path, e);
                if task.kind != ListingKind::Diff {
                    // Keep the folder visible (as an empty one) so its parent entry has a row.
                    let listing = DirListing::new(
                        task.id,
                        task.parent_id,
                        task.kind,
                        task.path,
                        meta.ino,
                        meta.mtime_ns,
                        0,
                    );
                    self.job.ctl.counters.listed.fetch_add(1, Ordering::Relaxed);
                    self.emit(listing);
                }
                return;
            }
        };

        let listing = SCRATCH.with_borrow_mut(|sc| self.list_with(sc, &fd, task, meta, children));
        if let Some(listing) = listing {
            let counters = &self.job.ctl.counters;
            counters.listed.fetch_add(1, Ordering::Relaxed);
            counters
                .entries
                .fetch_add(listing.entries.len() as u64, Ordering::Relaxed);
            self.emit(listing);
        }
    }

    fn list_with<'s>(
        &'s self,
        sc: &mut Scratch,
        fd: &rustix::fd::OwnedFd,
        task: Task<'s>,
        meta: DirMeta,
        children: &mut Vec<Task<'s>>,
    ) -> Option<DirListing> {
        // 1. Names and d_type, straight from the getdents buffer.
        sc.names.clear();
        sc.raw.clear();
        {
            let mut dir = RawDir::new(fd, &mut sc.dirents);
            while let Some(entry) = dir.next() {
                match entry {
                    Ok(e) => {
                        let name = e.file_name().to_bytes_with_nul();
                        if name == b".\0" || name == b"..\0" {
                            continue;
                        }
                        sc.raw.push(RawEnt {
                            ino: e.ino(),
                            name_off: u32::try_from(sc.names.len()).unwrap_or(u32::MAX),
                            name_len: u16::try_from(name.len() - 1).unwrap_or(u16::MAX),
                            file_type: e.file_type(),
                        });
                        sc.names.extend_from_slice(name);
                    }
                    Err(e) => {
                        self.count_error(&task.path, e);
                        // A folder we could only read partially must not be diffed: that would
                        // delete the entries we failed to see.
                        if task.kind == ListingKind::Diff {
                            return None;
                        }
                        break;
                    }
                }
            }
        }
        // 2. Stat in inode order: sequential inode-table access, a big win on HDDs.
        sc.raw.sort_unstable_by_key(|e| e.ino);

        let mut listing = DirListing::new(
            task.id,
            task.parent_id,
            task.kind,
            task.path,
            meta.ino,
            meta.mtime_ns,
            sc.raw.len(),
        );
        sc.path.clear();
        sc.path.extend_from_slice(&listing.path);
        if sc.path.last() != Some(&b'/') {
            sc.path.push(b'/');
        }
        let base = sc.path.len();

        for (i, raw) in sc.raw.iter().enumerate() {
            if i % CANCEL_CHECK_EVERY == CANCEL_CHECK_EVERY - 1 && self.cancelled() {
                return None;
            }
            let start = raw.name_off as usize;
            let end = start + raw.name_len as usize;
            let name = &sc.names[start..end];
            sc.path.truncate(base);
            sc.path.extend_from_slice(name);
            let Ok(name_c) = CStr::from_bytes_with_nul(&sc.names[start..=end]) else {
                continue;
            };

            // Prune by name when d_type already tells us what it is.
            let hinted_dir = match raw.file_type {
                FileType::Unknown => None,
                t => Some(t == FileType::Directory),
            };
            if self.has_exclusions
                && let Some(is_dir) = hinted_dir
                && self.job.opts.exclusions.is_excluded(path_of(&sc.path), is_dir)
            {
                continue;
            }

            let st = match statat(fd, name_c, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(st) => st,
                Err(Errno::NOENT) => continue, // deleted since readdir
                Err(e) => {
                    self.count_error(&sc.path, e);
                    let mut row = EntryRow::new(kind_of(raw.file_type), 0, 0, raw.ino, 0, 0);
                    // A folder we cannot stat is recorded but not entered.
                    row.child_dir = 0;
                    listing.push(name, row);
                    continue;
                }
            };
            let file_type = FileType::from_raw_mode(st.st_mode);
            let is_dir = file_type.is_dir();
            if self.has_exclusions
                && hinted_dir.is_none()
                && self.job.opts.exclusions.is_excluded(path_of(&sc.path), is_dir)
            {
                continue;
            }

            let kind = kind_of(file_type);
            let size = if is_dir {
                0
            } else {
                u64::try_from(st.st_size).unwrap_or(0)
            };
            let mut row = EntryRow::new(kind, size, mtime_ns(&st), st.st_ino, st.st_uid, st.st_mode);
            if is_dir && (!self.job.opts.one_filesystem || st.st_dev == self.root_dev) {
                row.child_dir = self.descend(&sc.path, task.known, &st, listing.id, children);
            }
            listing.push(name, row);
        }
        Some(listing)
    }

    /// Decides how a sub-folder is walked and queues its task. `sc.path` holds the sub-folder's
    /// path. Returns the id the sub-folder has in the database.
    fn descend<'s>(
        &'s self,
        path: &[u8],
        parent_known: Option<&'s KnownDir>,
        st: &Stat,
        parent_id: i64,
        children: &mut Vec<Task<'s>>,
    ) -> i64 {
        let known_child = match (self.job.mode, parent_known) {
            (WalkMode::Quick { known } | WalkMode::Shallow { known }, Some(_)) => known.get(path),
            _ => None,
        };
        let meta = Some(DirMeta::from_stat(st));
        match (self.job.mode, known_child) {
            // Shallow walks leave known sub-folders alone.
            (WalkMode::Shallow { .. }, Some(k)) => k.id,
            (_, Some(k)) => {
                children.push(Task {
                    path: path.to_vec(),
                    id: k.id,
                    parent_id: Some(parent_id),
                    kind: ListingKind::Diff,
                    meta,
                    known: Some(k),
                });
                k.id
            }
            (_, None) => {
                let id = self.job.ids.next();
                children.push(Task {
                    path: path.to_vec(),
                    id,
                    parent_id: Some(parent_id),
                    kind: ListingKind::Insert,
                    meta,
                    known: None,
                });
                id
            }
        }
    }
}

fn path_of(bytes: &[u8]) -> &Path {
    Path::new(OsStr::from_bytes(bytes))
}

const fn kind_of(t: FileType) -> EntryKind {
    match t {
        FileType::RegularFile => EntryKind::File,
        FileType::Directory => EntryKind::Dir,
        FileType::Symlink => EntryKind::Symlink,
        _ => EntryKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_convert_including_pre_epoch() {
        assert_eq!(to_i64(5u64), 5);
        assert_eq!(to_i64(-5i64), -5);
        assert_eq!(to_i64(u64::MAX - 4), -5, "wrapped unsigned = before 1970");
    }

    #[test]
    fn id_alloc_is_sequential() {
        let ids = IdAlloc::new(7);
        assert_eq!((ids.next(), ids.next()), (7, 8));
    }
}

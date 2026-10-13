//! Live updates via inotify in user mode (ROADMAP Phase 9).
//!
//! A [`Watcher`] puts a watch on every folder below the watch roots (honouring exclusions, within
//! a budget), collects change notifications for a debounce window (default 300 ms) and then
//! submits one `Job::RefreshDirs` with the folders that changed. New folders get watches as they
//! appear; a kernel queue overflow triggers a quick rescan of the roots instead.
//!
//! The thread waits in `poll(2)` on the inotify descriptor and an `eventfd`, so dropping the
//! [`Watcher`] wakes and joins it immediately.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use everythux_core::config::IndexerConfig;
use everythux_core::exclusion::ExclusionSet;
use everythux_core::model::{VolumeInfo, VolumeState};
use rustix::event::{EventfdFlags, PollFd, PollFlags, Timespec, eventfd, poll};
use rustix::fs::inotify::{self, CreateFlags, ReadFlags, Reader, WatchFlags};
use rustix::io::Errno;

use crate::IndexError;
use crate::indexer::{Job, ScanMode};

/// Default time events are collected before a refresh job is submitted.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(300);
/// Upper bound of the default watch budget.
const MAX_DEFAULT_WATCHES: usize = 200_000;

/// What to watch.
#[derive(Debug, Clone)]
pub struct WatchConfig {
    /// Folders watched recursively.
    pub roots: Vec<PathBuf>,
    /// Folders matching these rules are not watched.
    pub exclusions: ExclusionSet,
    /// Folders that are never watched (the index's own directory, to avoid feedback loops).
    pub ignore: Vec<PathBuf>,
    /// Maximum number of inotify watches this watcher may use.
    pub max_watches: usize,
    /// How long events are collected before a refresh is submitted.
    pub debounce: Duration,
}

impl WatchConfig {
    /// A config with the default budget and debounce time.
    pub fn new(roots: Vec<PathBuf>, exclusions: ExclusionSet) -> Self {
        Self {
            roots,
            exclusions,
            ignore: Vec::new(),
            max_watches: default_budget(),
            debounce: DEFAULT_DEBOUNCE,
        }
    }
}

/// Half of `fs.inotify.max_user_watches`, at most 200 000: other programs need watches too.
pub fn default_budget() -> usize {
    let limit = std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(8192);
    (limit / 2).clamp(1, MAX_DEFAULT_WATCHES)
}

/// The folders to watch: `indexer.watch_roots`, or `$HOME` when empty; only those that lie on an
/// indexed (not offline) volume, and without roots nested inside other roots.
pub fn effective_roots(config: &IndexerConfig, volumes: &[VolumeInfo]) -> Vec<PathBuf> {
    let candidates: Vec<PathBuf> = if config.watch_roots.is_empty() {
        std::env::var_os("HOME").map(PathBuf::from).into_iter().collect()
    } else {
        config.watch_roots.clone()
    };
    let indexed = |p: &Path| {
        volumes
            .iter()
            .any(|v| v.state != VolumeState::Offline && p.starts_with(&v.mount_point))
    };
    let mut roots: Vec<PathBuf> = candidates
        .into_iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .filter(|p| p.is_dir() && indexed(p))
        .collect();
    roots.sort();
    roots.dedup();
    let all = roots.clone();
    roots.retain(|r| !all.iter().any(|other| other != r && r.starts_with(other)));
    roots
}

/// Handle to the watcher thread. Dropping it stops and joins the thread.
#[derive(Debug)]
pub struct Watcher {
    stopping: Arc<AtomicBool>,
    wake: OwnedFd,
    thread: Option<JoinHandle<()>>,
}

type Submit = Arc<dyn Fn(Job) + Send + Sync>;

impl Watcher {
    /// Starts watching in a background thread. Watches are added asynchronously, so this returns
    /// quickly even for huge trees. Refresh jobs are handed to `submit`.
    pub fn start(config: WatchConfig, submit: Submit) -> Result<Self, IndexError> {
        let first_root = config.roots.first().cloned().unwrap_or_default();
        let to_err = |e: Errno| IndexError::errno(&first_root, e);
        let inotify = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK).map_err(to_err)?;
        let wake = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK).map_err(to_err)?;
        let thread_wake = wake.try_clone().map_err(|e| IndexError::io(&first_root, e))?;
        let stopping = Arc::new(AtomicBool::new(false));
        let engine = Engine {
            inotify,
            wake: thread_wake,
            stopping: Arc::clone(&stopping),
            watches: HashMap::new(),
            config,
            submit,
            warned_budget: false,
        };
        let thread = std::thread::Builder::new()
            .name("everythux-watch".to_owned())
            .spawn(move || engine.run())
            .map_err(|e| IndexError::io(&first_root, e))?;
        Ok(Self {
            stopping,
            wake,
            thread: Some(thread),
        })
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Err(e) = rustix::io::write(&self.wake, &1_u64.to_ne_bytes()) {
            tracing::debug!(error = %e, "cannot wake the watcher");
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::error!("watcher thread panicked");
        }
    }
}

/// One event copied out of the inotify buffer.
struct RawEvent {
    wd: i32,
    flags: ReadFlags,
    name: Option<OsString>,
}

struct Engine {
    inotify: OwnedFd,
    wake: OwnedFd,
    stopping: Arc<AtomicBool>,
    /// Watch descriptor → folder path.
    watches: HashMap<i32, PathBuf>,
    config: WatchConfig,
    submit: Submit,
    warned_budget: bool,
}

const WATCH_MASK: WatchFlags = WatchFlags::CREATE
    .union(WatchFlags::DELETE)
    .union(WatchFlags::MOVED_FROM)
    .union(WatchFlags::MOVED_TO)
    .union(WatchFlags::CLOSE_WRITE)
    .union(WatchFlags::ATTRIB)
    .union(WatchFlags::DELETE_SELF)
    .union(WatchFlags::MOVE_SELF)
    .union(WatchFlags::ONLYDIR)
    .union(WatchFlags::DONT_FOLLOW)
    .union(WatchFlags::EXCL_UNLINK);

impl Engine {
    fn stop_requested(&self) -> bool {
        self.stopping.load(Ordering::Relaxed)
    }

    fn run(mut self) {
        for root in self.config.roots.clone() {
            self.add_tree(&root);
        }
        tracing::debug!(watches = self.watches.len(), "inotify watches established");

        let mut dirty: HashSet<PathBuf> = HashSet::new();
        let mut overflow = false;
        let mut deadline: Option<Instant> = None;
        while !self.stop_requested() {
            let timeout = deadline.map(|d| {
                let left = d.saturating_duration_since(Instant::now());
                Timespec {
                    tv_sec: i64::try_from(left.as_secs()).unwrap_or(i64::MAX),
                    tv_nsec: i64::from(left.subsec_nanos()),
                }
            });
            let mut fds = [
                PollFd::new(&self.inotify, PollFlags::IN),
                PollFd::new(&self.wake, PollFlags::IN),
            ];
            match poll(&mut fds, timeout.as_ref()) {
                Ok(_) => {}
                Err(Errno::INTR) => continue,
                Err(e) => {
                    tracing::error!(error = %e, "watcher poll failed; live updates stopped");
                    return;
                }
            }
            if fds[1].revents().contains(PollFlags::IN) {
                return;
            }
            if fds[0].revents().contains(PollFlags::IN) {
                for event in self.read_events() {
                    self.handle(event, &mut dirty, &mut overflow);
                }
                if (overflow || !dirty.is_empty()) && deadline.is_none() {
                    deadline = Some(Instant::now() + self.config.debounce);
                }
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                deadline = None;
                self.flush(&mut dirty, &mut overflow);
            }
        }
    }

    /// Drains the inotify queue.
    fn read_events(&self) -> Vec<RawEvent> {
        let mut buf = [MaybeUninit::<u8>::uninit(); 16 * 1024];
        let mut reader = Reader::new(&self.inotify, &mut buf);
        let mut events = Vec::new();
        loop {
            match reader.next() {
                Ok(event) => events.push(RawEvent {
                    wd: event.wd(),
                    flags: event.events(),
                    name: event
                        .file_name()
                        .map(|n| OsString::from_vec(n.to_bytes().to_vec())),
                }),
                Err(Errno::AGAIN) => break,
                Err(e) => {
                    tracing::debug!(error = %e, "reading inotify events failed");
                    break;
                }
            }
        }
        events
    }

    fn handle(&mut self, event: RawEvent, dirty: &mut HashSet<PathBuf>, overflow: &mut bool) {
        if event.flags.contains(ReadFlags::QUEUE_OVERFLOW) {
            *overflow = true;
            return;
        }
        let Some(dir) = self.watches.get(&event.wd).cloned() else {
            return;
        };
        if event.flags.contains(ReadFlags::IGNORED) {
            self.watches.remove(&event.wd);
            return;
        }
        if let Some(name) = event.name {
            if event.flags.contains(ReadFlags::ISDIR)
                && event.flags.intersects(ReadFlags::CREATE | ReadFlags::MOVED_TO)
            {
                self.add_tree(&dir.join(&name));
            }
            dirty.insert(dir);
        } else {
            // The watched folder itself was deleted, moved or changed: its parent's listing
            // (name, mtime, mode) is what is stale.
            if let Some(parent) = dir.parent() {
                dirty.insert(parent.to_path_buf());
            }
            if !event
                .flags
                .intersects(ReadFlags::DELETE_SELF | ReadFlags::MOVE_SELF)
            {
                dirty.insert(dir);
            }
        }
    }

    fn flush(&mut self, dirty: &mut HashSet<PathBuf>, overflow: &mut bool) {
        if std::mem::take(overflow) {
            // Events were lost: re-establish watches and let a quick rescan find the differences.
            dirty.clear();
            for root in self.config.roots.clone() {
                self.add_tree(&root);
                (self.submit)(Job::RescanPath {
                    path: root,
                    mode: ScanMode::Quick,
                });
            }
            return;
        }
        if dirty.is_empty() {
            return;
        }
        let mut dirs: Vec<PathBuf> = dirty.drain().collect();
        dirs.sort_unstable();
        (self.submit)(Job::RefreshDirs(dirs));
    }

    /// Watches `root` and every folder below it that is not excluded, within the budget.
    fn add_tree(&mut self, root: &Path) {
        let mut stack = vec![root.to_path_buf()];
        let mut visited = 0_u32;
        while let Some(dir) = stack.pop() {
            visited += 1;
            if visited.is_multiple_of(256) && self.stop_requested() {
                return;
            }
            if self.config.ignore.iter().any(|i| dir.starts_with(i)) {
                continue;
            }
            if dir != root && self.config.exclusions.is_excluded(&dir, true) {
                continue;
            }
            if !self.add_one(&dir) {
                if self.watches.len() >= self.config.max_watches {
                    return;
                }
                continue;
            }
            let Ok(read) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in read.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(dir.join(entry.file_name()));
                }
            }
        }
    }

    /// Adds one watch. Returns false if it could not be added (budget, permissions, vanished).
    fn add_one(&mut self, dir: &Path) -> bool {
        if self.watches.len() >= self.config.max_watches {
            if !self.warned_budget {
                self.warned_budget = true;
                tracing::warn!(
                    budget = self.config.max_watches,
                    "inotify watch budget exhausted; the rest of the tree relies on rescans"
                );
            }
            return false;
        }
        match inotify::add_watch(&self.inotify, dir.as_os_str().as_bytes(), WATCH_MASK) {
            Ok(wd) => {
                // The same inode watched under a new path (a moved folder) keeps its wd.
                self.watches.insert(wd, dir.to_path_buf());
                true
            }
            Err(Errno::NOSPC) => {
                if !self.warned_budget {
                    self.warned_budget = true;
                    tracing::warn!(
                        "system inotify watch limit reached; the rest of the tree relies on rescans"
                    );
                }
                // Pretend the budget is spent so the walk stops trying.
                self.config.max_watches = self.watches.len();
                false
            }
            Err(e) => {
                tracing::debug!(dir = %dir.display(), error = %e, "cannot watch folder");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(mount: &Path, state: VolumeState) -> VolumeInfo {
        VolumeInfo {
            fs_uuid: "u".to_owned(),
            device: PathBuf::new(),
            mount_point: mount.to_path_buf(),
            label: None,
            fs_type: "ext4".to_owned(),
            state,
            last_full_scan_ns: None,
            last_scan_ms: None,
        }
    }

    #[test]
    fn roots_must_be_inside_online_volumes_and_not_nested() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().canonicalize().expect("canonicalize");
        let inner = base.join("a/b");
        std::fs::create_dir_all(&inner).expect("mkdir");
        let config = IndexerConfig {
            watch_roots: vec![
                base.join("a"),
                inner.clone(),
                PathBuf::from("/definitely/not/here"),
            ],
            ..IndexerConfig::default()
        };
        let online = [volume(&base, VolumeState::Online)];
        assert_eq!(effective_roots(&config, &online), [base.join("a")]);
        let offline = [volume(&base, VolumeState::Offline)];
        assert_eq!(effective_roots(&config, &offline), [] as [PathBuf; 0]);
        assert_eq!(effective_roots(&config, &[]), [] as [PathBuf; 0]);
    }

    #[test]
    fn budget_is_bounded() {
        let b = default_budget();
        assert!((1..=MAX_DEFAULT_WATCHES).contains(&b), "{b}");
    }
}

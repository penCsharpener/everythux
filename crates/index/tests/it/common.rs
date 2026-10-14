//! Helpers shared by the integration tests.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use everythux_core::config::{IndexerConfig, StartupRescan, VolumesConfig};
use everythux_core::ids::DirId;
use everythux_core::model::EntryKind;
use everythux_core::snapshot::IndexSnapshot;
use everythux_index::{IndexEvent, Indexer, IndexerSettings, Job, JobId, JobStats};

/// How long a test waits for the indexer before failing.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(60);

/// Settings that index exactly `roots` and nothing else on the machine.
pub(crate) fn settings(roots: &[&Path]) -> IndexerSettings {
    IndexerSettings {
        volumes: VolumesConfig {
            auto_discover_fs_types: vec![],
            include: roots.iter().map(|p| p.to_path_buf()).collect(),
            disabled_uuids: vec![],
        },
        watch_ignore: Vec::new(),
        exclusions: vec![],
        indexer: IndexerConfig {
            threads: 4,
            rescan_on_startup: StartupRescan::None,
            live_updates: false,
            ..IndexerConfig::default()
        },
    }
}

/// How a job ended.
#[derive(Debug)]
pub(crate) enum Done {
    Finished(JobStats),
    Failed(String),
    Cancelled,
}

impl Done {
    pub(crate) fn stats(self) -> JobStats {
        match self {
            Self::Finished(s) => s,
            other => panic!("job did not finish: {other:?}"),
        }
    }
}

/// Waits until `pick` returns `Some`, panicking after [`TIMEOUT`].
pub(crate) fn wait_for<T>(
    rx: &Receiver<IndexEvent>,
    what: &str,
    mut pick: impl FnMut(&IndexEvent) -> Option<T>,
) -> T {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(event) => {
                if let Some(found) = pick(&event) {
                    return found;
                }
            }
            Err(e) => panic!("timed out waiting for {what}: {e}"),
        }
    }
}

/// Waits for the end of job `id`.
pub(crate) fn wait_done(rx: &Receiver<IndexEvent>, id: JobId) -> Done {
    wait_for(rx, "job end", |e| match e {
        IndexEvent::Finished { job, stats } if *job == id => Some(Done::Finished(stats.clone())),
        IndexEvent::Failed { job, error } if *job == id => Some(Done::Failed(error.clone())),
        IndexEvent::Cancelled { job } if *job == id => Some(Done::Cancelled),
        _ => None,
    })
}

/// Submits a job and waits for it.
pub(crate) fn run(indexer: &Indexer, rx: &Receiver<IndexEvent>, job: Job) -> Done {
    let id = indexer.submit(job);
    wait_done(rx, id)
}

/// Submits a job that must succeed.
pub(crate) fn run_ok(indexer: &Indexer, rx: &Receiver<IndexEvent>, job: Job) -> JobStats {
    run(indexer, rx, job).stats()
}

/// Starts an indexer on a fresh database in `db_dir`, waits for the initial full scan
/// (the database is empty) and returns it.
pub(crate) fn start_scanned(db_dir: &Path, settings: IndexerSettings) -> (Indexer, Receiver<IndexEvent>) {
    let (indexer, rx) = Indexer::start(&db_dir.join("index.db"), settings).expect("start");
    wait_for(&rx, "initial scan", |e| match e {
        IndexEvent::Finished { .. } => Some(()),
        IndexEvent::Failed { error, .. } => panic!("initial scan failed: {error}"),
        _ => None,
    });
    (indexer, rx)
}

/// One entry of [`listing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) kind: EntryKind,
    pub(crate) size: u64,
    pub(crate) ext: String,
}

/// All entries of a snapshot by path relative to `root` (raw bytes shown lossily).
pub(crate) fn listing(snap: &IndexSnapshot, root: &Path) -> BTreeMap<String, Row> {
    let mut out = BTreeMap::new();
    for i in 0..snap.entries.len() {
        let id = everythux_core::ids::EntryId(u32::try_from(i).expect("fits"));
        let full = snap.full_path(id);
        let Ok(rel) = full.strip_prefix(root) else {
            continue;
        };
        out.insert(
            rel.to_string_lossy().into_owned(),
            Row {
                kind: snap.entries.kind[i],
                size: snap.entries.size[i],
                ext: snap.extension(id).to_owned(),
            },
        );
    }
    out
}

/// `(size, kind)` of a relative path, if indexed.
pub(crate) fn row(snap: &IndexSnapshot, root: &Path, rel: &str) -> Option<Row> {
    listing(snap, root).remove(rel)
}

/// Recursive size of the indexed folder at `path`.
pub(crate) fn dir_total(snap: &IndexSnapshot, path: &Path) -> Option<u64> {
    (0..snap.dirs.len())
        .find(|&i| snap.dirs.path(DirId(u32::try_from(i).expect("fits"))) == path)
        .map(|i| snap.dirs.total_size[i])
}

/// Folder sizes by path, for comparing two snapshots of the same tree.
pub(crate) fn dir_totals(snap: &IndexSnapshot, root: &Path) -> BTreeMap<String, u64> {
    (0..snap.dirs.len())
        .filter_map(|i| {
            let p = snap.dirs.path(DirId(u32::try_from(i).expect("fits")));
            let rel = p.strip_prefix(root).ok()?;
            Some((rel.to_string_lossy().into_owned(), snap.dirs.total_size[i]))
        })
        .collect()
}

/// Structural invariants every snapshot must satisfy.
pub(crate) fn check_invariants(snap: &IndexSnapshot) {
    let (d, e) = (&snap.dirs, &snap.entries);
    assert_eq!(
        snap.extensions.first().map(|s| &**s),
        Some(""),
        "extension 0 is empty"
    );
    let mut files_in: Vec<u64> = vec![0; d.len()];
    let mut subdirs_in: Vec<u64> = vec![0; d.len()];
    for i in 0..e.len() {
        assert!(e.dir[i].index() < d.len(), "entry {i} has a valid folder");
        assert!((e.ext[i] as usize) < snap.extensions.len());
        let sd = e.self_dir[i];
        if e.kind[i] == EntryKind::Dir && !sd.is_none() {
            assert_eq!(d.entry[sd.index()].index(), i, "folder entry {i} links back");
            assert_eq!(
                e.size[i],
                d.total_size[sd.index()],
                "folder entry size is the folder total"
            );
            subdirs_in[e.dir[i].index()] += d.total_size[sd.index()];
        } else if e.kind[i] != EntryKind::Dir {
            assert!(sd.is_none(), "only folders have a self dir");
            files_in[e.dir[i].index()] += e.size[i];
        }
    }
    for i in 0..d.len() {
        assert_eq!(
            d.total_size[i],
            files_in[i] + subdirs_in[i],
            "total_size of {}",
            d.path(DirId(u32::try_from(i).expect("fits"))).display()
        );
        if !d.parent[i].is_none() {
            assert!(d.parent[i].index() < d.len());
            let parent = d.path(d.parent[i]);
            assert_eq!(
                d.path(DirId(u32::try_from(i).expect("fits"))).parent(),
                Some(parent)
            );
        }
        if !d.entry[i].is_none() {
            assert_eq!(e.self_dir[d.entry[i].index()].index(), i);
        }
    }
}

/// Path made of raw bytes.
pub(crate) fn raw(bytes: &[u8]) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(bytes))
}

/// True when running as root (permission tests can't observe denials then).
pub(crate) fn is_root() -> bool {
    rustix::process::geteuid().is_root()
}

/// Polls `indexer.snapshot()` until `ok` holds. Live-update jobs publish their snapshot
/// asynchronously (rate limited), so their `Finished` can precede it.
pub(crate) fn eventually(
    indexer: &Indexer,
    what: &str,
    ok: impl Fn(&IndexSnapshot) -> bool,
) -> std::sync::Arc<IndexSnapshot> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let snap = indexer.snapshot();
        if ok(&snap) {
            return snap;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

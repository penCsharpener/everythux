//! The jobs themselves: each function runs on the worker thread with exclusive access to the
//! writer [`Store`], drives a walk and feeds its batches into the database.

use std::collections::BTreeSet;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossbeam_channel::{RecvTimeoutError, Sender, bounded};
use everythux_core::exclusion::ExclusionSet;
use everythux_core::model::{VolumeInfo, VolumeState};

use super::shared::{Shared, millis};
use super::{IndexEvent, IndexerSettings, JobId, Progress, ScanMode};
use crate::IndexError;
use crate::discovery::{DiscoveredVolume, discover_volumes};
use crate::store::{ApplyStats, Store};
use crate::walker::{
    IdAlloc, RootRef, ScanBatch, WalkControl, WalkCounters, WalkJob, WalkMode, WalkOptions, WalkSummary,
};

/// Minimum spacing of progress events (≤ 10 per second).
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// Everything a job needs besides the store.
#[derive(Debug)]
pub(super) struct Ctx<'a> {
    pub(super) job: JobId,
    pub(super) events: &'a Sender<IndexEvent>,
    pub(super) shared: &'a Shared,
    pub(super) settings: &'a IndexerSettings,
    pub(super) exclusions: &'a ExclusionSet,
    pub(super) pool: Option<&'a rayon::ThreadPool>,
    /// Entries in the index before this job started (for the progress fraction of full scans).
    pub(super) expected_entries: Option<u64>,
}

/// What a job did, for the `Finished` event.
#[derive(Debug, Default)]
pub(super) struct Outcome {
    pub(super) changed: bool,
    pub(super) upserted: u64,
    pub(super) removed: u64,
    pub(super) dirs_listed: u64,
    pub(super) errors: u64,
}

impl Outcome {
    fn add_store(&mut self, s: ApplyStats) {
        self.upserted += s.upserted;
        self.removed += s.removed;
        self.changed |= s.upserted > 0 || s.removed > 0;
    }
}

/// Accumulates counters over the walks of one job and throttles progress events.
#[derive(Debug)]
pub(super) struct Tracker {
    done: WalkSummary,
    live: Option<Arc<WalkCounters>>,
    volume: Option<PathBuf>,
    current: PathBuf,
    last_sent: Instant,
    /// A `VolumesChanged` showing a volume as scanning was sent; the final state must follow.
    pub(super) announced_scanning: bool,
}

impl Tracker {
    /// Announces the volume list with the volume `fs_uuid` shown as [`VolumeState::Scanning`].
    fn announce_scanning(&mut self, ctx: &Ctx<'_>, store: &Store, fs_uuid: &str) {
        if let Ok(mut volumes) = store.volumes() {
            for v in &mut volumes {
                if v.fs_uuid == fs_uuid {
                    v.state = VolumeState::Scanning;
                }
            }
            self.announced_scanning = true;
            let _ = ctx.events.send(IndexEvent::VolumesChanged(volumes));
        }
    }

    pub(super) fn new() -> Self {
        Self {
            done: WalkSummary::default(),
            live: None,
            volume: None,
            current: PathBuf::new(),
            last_sent: Instant::now(),
            announced_scanning: false,
        }
    }

    fn summary(&self) -> WalkSummary {
        let live = self.live.as_ref().map(|c| c.summary()).unwrap_or_default();
        WalkSummary {
            dirs: self.done.dirs + live.dirs,
            listed: self.done.listed + live.listed,
            entries: self.done.entries + live.entries,
            errors: self.done.errors + live.errors,
        }
    }

    fn tick(&mut self, ctx: &Ctx<'_>) {
        if self.last_sent.elapsed() < PROGRESS_EVERY {
            return;
        }
        self.last_sent = Instant::now();
        let s = self.summary();
        let fraction = ctx
            .expected_entries
            .filter(|&n| n > 0)
            .map(|n| (s.entries as f32 / n as f32).min(0.99));
        // A closed event channel just means nobody listens anymore.
        let _ = ctx.events.send(IndexEvent::Progress {
            job: ctx.job,
            progress: Progress {
                volume: self.volume.clone(),
                dirs: s.dirs,
                entries: s.entries,
                errors: s.errors,
                current: self.current.clone(),
                fraction,
            },
        });
    }
}

impl Ctx<'_> {
    fn walk_options(&self) -> WalkOptions {
        WalkOptions {
            exclusions: self.exclusions.clone(),
            one_filesystem: self.settings.indexer.one_filesystem,
            batch_rows: 10_000,
        }
    }

    fn cancelled(&self) -> bool {
        self.shared.cancel.load(Ordering::Relaxed)
    }
}

/// Runs one walk and feeds its batches to `apply` on this thread.
///
/// Returns when the walk finished, [`IndexError::Cancelled`] if the job was cancelled, or the first
/// error `apply` produced.
fn drive(
    ctx: &Ctx<'_>,
    tracker: &mut Tracker,
    root: &Path,
    mode: &WalkMode,
    ids: &IdAlloc,
    mut apply: impl FnMut(ScanBatch) -> Result<(), IndexError>,
) -> Result<WalkSummary, IndexError> {
    let opts = ctx.walk_options();
    let ctl = WalkControl {
        cancel: Arc::clone(&ctx.shared.cancel),
        counters: Arc::new(WalkCounters::default()),
    };
    tracker.live = Some(Arc::clone(&ctl.counters));
    let job = WalkJob {
        root,
        mode,
        opts: &opts,
        ids,
        ctl: &ctl,
        pool: ctx.pool,
    };

    // Bounded: if SQLite is slower than the walker, the walker blocks instead of buffering.
    let (tx, rx) = bounded::<ScanBatch>(4);
    let mut apply_error = None;
    let walked = std::thread::scope(|scope| {
        let walker = scope.spawn(|| {
            let result = job.run(&tx);
            drop(tx);
            result
        });
        loop {
            match rx.recv_timeout(PROGRESS_EVERY) {
                Ok(batch) => {
                    if let Some(last) = batch.listings.last() {
                        tracker.current = PathBuf::from(std::ffi::OsStr::from_bytes(&last.path));
                    }
                    if let Err(e) = apply(batch) {
                        apply_error = Some(e);
                        ctx.shared.cancel.store(true, Ordering::Relaxed);
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            tracker.tick(ctx);
            if ctx.cancelled() {
                break;
            }
        }
        // Unblock a walker waiting on a full channel.
        drop(rx);
        walker.join()
    });
    let summary = ctl.counters.summary();
    tracker.live = None;
    tracker.done = WalkSummary {
        dirs: tracker.done.dirs + summary.dirs,
        listed: tracker.done.listed + summary.listed,
        entries: tracker.done.entries + summary.entries,
        errors: tracker.done.errors + summary.errors,
    };
    let walked = walked.map_err(|_| IndexError::Internal("walker thread panicked".to_owned()))?;
    if let Some(e) = apply_error {
        return Err(e);
    }
    walked?;
    if ctx.cancelled() {
        return Err(IndexError::Cancelled);
    }
    Ok(summary)
}

fn finish(tracker: &Tracker, mut out: Outcome) -> Outcome {
    let s = tracker.summary();
    out.dirs_listed = s.listed;
    out.errors = s.errors;
    out
}

/// Rebuilds the whole database from scratch (`Ctrl+Shift+R`).
pub(super) fn full_reindex(
    ctx: &Ctx<'_>,
    store: &mut Store,
    tracker: &mut Tracker,
) -> Result<Outcome, IndexError> {
    let discovered = discover_volumes(&ctx.settings.volumes)?;
    store.sync_volumes(&discovered)?;
    let mut rebuild = store.begin_rebuild(&discovered)?;
    let ids = rebuild.ids();
    let mut failed: Vec<&DiscoveredVolume> = Vec::new();
    for volume in &discovered {
        let id = rebuild.add_volume(volume)?;
        tracker.announce_scanning(ctx, store, &volume.fs_uuid);
        tracker.volume = Some(volume.mount_point.clone());
        let started = Instant::now();
        let mode = WalkMode::Fresh { existing_root: None };
        match drive(ctx, tracker, &volume.mount_point, &mode, &ids, |b| {
            rebuild.apply(id, b)
        }) {
            Ok(_) => rebuild.finish_volume(id, millis(started.elapsed()))?,
            Err(e @ IndexError::Io { .. }) => {
                tracing::warn!(volume = %volume.mount_point.display(), error = %e, "cannot scan volume");
                failed.push(volume);
            }
            Err(e) => return Err(e),
        }
    }
    let stats = rebuild.finish(store)?;
    for volume in failed {
        store.set_volume_state(&volume.fs_uuid, VolumeState::Error)?;
    }
    let mut out = Outcome::default();
    out.add_store(stats);
    out.changed = true;
    Ok(finish(tracker, out))
}

/// Rescans every online volume after re-discovering what is mounted.
pub(super) fn rescan_all(
    ctx: &Ctx<'_>,
    store: &mut Store,
    tracker: &mut Tracker,
    mode: ScanMode,
) -> Result<Outcome, IndexError> {
    let discovered = discover_volumes(&ctx.settings.volumes)?;
    let volumes = store.sync_volumes(&discovered)?;
    let mut out = Outcome::default();
    for volume in volumes.iter().filter(|v| v.state == VolumeState::Online) {
        let stats = scan_volume(ctx, store, tracker, volume, mode)?;
        out.add_store(stats);
        // Each volume is its own transaction: a later cancel keeps the earlier volumes' work.
    }
    Ok(finish(tracker, out))
}

/// Looks a volume up by filesystem UUID or mount point.
fn find_volume(store: &Store, ident: &str) -> Result<VolumeInfo, IndexError> {
    let volumes = store.volumes()?;
    let found = volumes
        .into_iter()
        .find(|v| v.fs_uuid == ident || v.mount_point == Path::new(ident));
    match found {
        Some(v) if v.state != VolumeState::Offline => Ok(v),
        Some(_) => Err(IndexError::UnknownVolume(format!("{ident} (not mounted)"))),
        None => Err(IndexError::UnknownVolume(ident.to_owned())),
    }
}

pub(super) fn rescan_volume(
    ctx: &Ctx<'_>,
    store: &mut Store,
    tracker: &mut Tracker,
    ident: &str,
    mode: ScanMode,
) -> Result<Outcome, IndexError> {
    let volume = find_volume(store, ident)?;
    let stats = scan_volume(ctx, store, tracker, &volume, mode)?;
    let mut out = Outcome::default();
    out.add_store(stats);
    Ok(finish(tracker, out))
}

/// Rescans one whole volume in a single transaction.
fn scan_volume(
    ctx: &Ctx<'_>,
    store: &mut Store,
    tracker: &mut Tracker,
    volume: &VolumeInfo,
    mode: ScanMode,
) -> Result<ApplyStats, IndexError> {
    tracker.announce_scanning(ctx, store, &volume.fs_uuid);
    tracker.volume = Some(volume.mount_point.clone());
    let started = Instant::now();
    let mut update = store.begin_update()?;
    let volume_id = update
        .volume_id(&volume.fs_uuid)?
        .ok_or_else(|| IndexError::UnknownVolume(volume.fs_uuid.clone()))?;
    let root = volume.mount_point.as_path();
    let existing = update.find_dir(volume_id, root)?;

    let (walk_mode, counts_as_full) = match (mode, existing) {
        (ScanMode::Quick, Some(_)) => {
            let known = Arc::new(update.known_tree(volume_id, root, true)?);
            (WalkMode::Quick { known }, false)
        }
        (ScanMode::Full, Some(dir)) => {
            update.prepare_replace(dir.id)?;
            let existing_root = Some(RootRef {
                id: dir.id,
                parent_id: dir.parent_id,
            });
            (WalkMode::Fresh { existing_root }, true)
        }
        // Never scanned: both modes are a full first scan.
        (_, None) => (WalkMode::Fresh { existing_root: None }, true),
    };
    let ids = update.ids();
    drive(ctx, tracker, root, &walk_mode, &ids, |b| {
        update.apply(volume_id, b)
    })?;
    update.record_scan(volume_id, counts_as_full, millis(started.elapsed()))?;
    update.commit()
}

/// Rescans one folder subtree.
pub(super) fn rescan_path(
    ctx: &Ctx<'_>,
    store: &mut Store,
    tracker: &mut Tracker,
    path: &Path,
    mode: ScanMode,
) -> Result<Outcome, IndexError> {
    let volume = owning_volume(store, path)?;
    if path == volume.mount_point {
        let stats = scan_volume(ctx, store, tracker, &volume, mode)?;
        let mut out = Outcome::default();
        out.add_store(stats);
        return Ok(finish(tracker, out));
    }
    tracker.volume = Some(volume.mount_point.clone());
    let mut update = store.begin_update()?;
    let volume_id = update
        .volume_id(&volume.fs_uuid)?
        .ok_or_else(|| IndexError::UnknownVolume(volume.fs_uuid.clone()))?;

    // What the caller named may be gone, or a file: resolve to a folder that exists.
    let target = match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => path.to_path_buf(),
        Ok(_) => path
            .parent()
            .map_or_else(|| path.to_path_buf(), Path::to_path_buf),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(dir) = update.find_dir(volume_id, path)? {
                update.remove_dir(dir.id)?;
            }
            let mut out = Outcome::default();
            out.add_store(update.commit()?);
            return Ok(finish(tracker, out));
        }
        Err(e) => return Err(IndexError::io(path, e)),
    };

    let ids = update.ids();
    let (root, walk_mode) = match (mode, update.find_dir(volume_id, &target)?) {
        (ScanMode::Full, Some(dir)) => {
            update.prepare_replace(dir.id)?;
            let existing_root = Some(RootRef {
                id: dir.id,
                parent_id: dir.parent_id,
            });
            (target, WalkMode::Fresh { existing_root })
        }
        (ScanMode::Quick, Some(_)) => {
            let known = Arc::new(update.known_tree(volume_id, &target, true)?);
            (target, WalkMode::Quick { known })
        }
        // Not indexed yet: refresh the nearest indexed ancestor; new folders are listed fully.
        (_, None) => {
            let Some((ancestor, _)) = update.nearest_known_dir(volume_id, &target)? else {
                drop(update);
                let stats = scan_volume(ctx, store, tracker, &volume, mode)?;
                let mut out = Outcome::default();
                out.add_store(stats);
                return Ok(finish(tracker, out));
            };
            let known = Arc::new(update.known_tree(volume_id, &ancestor, false)?);
            (ancestor, WalkMode::Shallow { known })
        }
    };
    drive(ctx, tracker, &root, &walk_mode, &ids, |b| {
        update.apply(volume_id, b)
    })?;
    let mut out = Outcome::default();
    out.add_store(update.commit()?);
    Ok(finish(tracker, out))
}

/// The online volume whose mount point is the longest prefix of `path`.
fn owning_volume(store: &Store, path: &Path) -> Result<VolumeInfo, IndexError> {
    store
        .volumes()?
        .into_iter()
        .filter(|v| v.state != VolumeState::Offline && path.starts_with(&v.mount_point))
        .max_by_key(|v| v.mount_point.as_os_str().len())
        .ok_or_else(|| IndexError::UnknownVolume(path.display().to_string()))
}

/// Deletes everything the current exclusions match (no disk access).
pub(super) fn apply_exclusions(ctx: &Ctx<'_>, store: &mut Store) -> Result<Outcome, IndexError> {
    let mut update = store.begin_update()?;
    update.apply_exclusions(ctx.exclusions)?;
    let mut out = Outcome::default();
    out.add_store(update.commit()?);
    Ok(out)
}

/// Re-lists exactly these folders (live-update batches).
pub(super) fn refresh_dirs(
    ctx: &Ctx<'_>,
    store: &mut Store,
    tracker: &mut Tracker,
    paths: &[PathBuf],
) -> Result<Outcome, IndexError> {
    let volumes: Vec<VolumeInfo> = store
        .volumes()?
        .into_iter()
        .filter(|v| v.state != VolumeState::Offline)
        .collect();
    let mut update = store.begin_update()?;

    // Resolve every path to (volume row, folder to re-list) first; removals happen right away.
    let mut targets: BTreeSet<(i64, PathBuf)> = BTreeSet::new();
    for path in paths {
        let Some(volume) = volumes
            .iter()
            .filter(|v| path.starts_with(&v.mount_point))
            .max_by_key(|v| v.mount_point.as_os_str().len())
        else {
            tracing::debug!(path = %path.display(), "refresh outside any indexed volume");
            continue;
        };
        let Some(volume_id) = update.volume_id(&volume.fs_uuid)? else {
            continue;
        };
        let folder = match std::fs::symlink_metadata(path) {
            Ok(m) if m.is_dir() => path.clone(),
            Ok(_) => path.parent().map_or_else(|| path.clone(), Path::to_path_buf),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(dir) = update.find_dir(volume_id, path)? {
                    update.remove_dir(dir.id)?;
                }
                // Its parent lost an entry; re-list it too.
                path.parent().map_or_else(|| path.clone(), Path::to_path_buf)
            }
            Err(e) => {
                tracing::debug!(path = %path.display(), error = %e, "cannot refresh");
                continue;
            }
        };
        if !folder.starts_with(&volume.mount_point) {
            continue;
        }
        // Without an indexed ancestor the volume was never scanned: a full scan will pick this up.
        let Some((target, _)) = update.nearest_known_dir(volume_id, &folder)? else {
            continue;
        };
        targets.insert((volume_id, target));
    }

    for (volume_id, folder) in &targets {
        if ctx.cancelled() {
            return Err(IndexError::Cancelled);
        }
        let known = Arc::new(update.known_tree(*volume_id, folder, false)?);
        let mode = WalkMode::Shallow { known };
        let ids = update.ids();
        match drive(ctx, tracker, folder, &mode, &ids, |b| update.apply(*volume_id, b)) {
            Ok(_) => {}
            // The folder vanished between resolving and walking: its parent's refresh covers it.
            Err(IndexError::Io { path, source }) if source.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(path = %path.display(), "folder vanished before refresh");
            }
            Err(e) => return Err(e),
        }
    }
    let mut out = Outcome::default();
    out.add_store(update.commit()?);
    Ok(finish(tracker, out))
}

//! The indexer thread: owns the writer [`Store`], runs jobs one at a time and publishes snapshots.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use everythux_core::exclusion::{ExclusionRule, ExclusionSet};
use everythux_core::model::VolumeInfo;

use super::jobs::{self, Ctx, Outcome, Tracker};
use super::shared::{Shared, Work};
use super::{IndexEvent, IndexerSettings, Job, JobStats, ScanMode};
use crate::IndexError;
use crate::discovery::discover_volumes;
use crate::store::Store;
use crate::walker::make_pool;
use crate::watch::{WatchConfig, Watcher, effective_roots};

/// Snapshot reloads are throttled for live-update jobs: never more often than this …
const MIN_PUBLISH_INTERVAL: Duration = Duration::from_millis(200);
/// … and never more often than this many times the duration of the last reload.
const PUBLISH_LOAD_FACTOR: u32 = 4;

#[derive(Debug)]
pub(super) struct Worker {
    pub(super) shared: Arc<Shared>,
    pub(super) store: Store,
    pub(super) events: Sender<IndexEvent>,
    pub(super) settings: IndexerSettings,
    pub(super) exclusions: ExclusionSet,
    pub(super) pool: Option<rayon::ThreadPool>,
    pub(super) watcher: Option<Watcher>,
    pub(super) volumes_sent: Vec<VolumeInfo>,
    /// Data changed since the last published snapshot.
    pub(super) dirty: bool,
    pub(super) last_publish: Instant,
    pub(super) last_load: Duration,
    pub(super) last_scheduled: Instant,
}

/// Compiles `rules`, skipping (and logging) the ones that are invalid instead of failing.
pub(super) fn compile_lenient(rules: &[ExclusionRule]) -> ExclusionSet {
    match ExclusionSet::compile(rules) {
        Ok(set) => set,
        Err(e) => {
            tracing::warn!(error = %e, "invalid exclusion rules; skipping the broken ones");
            let valid: Vec<ExclusionRule> = rules
                .iter()
                .filter(|r| match r.validate() {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::warn!(pattern = %r.pattern, error = %e, "skipping invalid exclusion rule");
                        false
                    }
                })
                .cloned()
                .collect();
            ExclusionSet::compile(&valid).unwrap_or_default()
        }
    }
}

/// `error` and all its causes, for `Failed` events.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

fn describe(job: &Job) -> String {
    let how = |mode: &ScanMode| match mode {
        ScanMode::Quick => "quick",
        ScanMode::Full => "full",
    };
    match job {
        Job::FullReindex => "Rebuilding the index".to_owned(),
        Job::RescanAll(mode) => format!("Rescanning all volumes ({})", how(mode)),
        Job::RescanVolume { volume, mode } => format!("Rescanning {volume} ({})", how(mode)),
        Job::RescanPath { path, mode } => format!("Rescanning {} ({})", path.display(), how(mode)),
        Job::ApplyExclusions => "Applying exclusions".to_owned(),
        Job::RefreshDirs(dirs) => format!("Refreshing {} folders", dirs.len()),
        Job::Renamed { from, .. } => format!("Renaming {}", from.display()),
    }
}

impl Worker {
    pub(super) fn run(mut self) {
        self.startup();
        loop {
            match self.shared.next_work(self.next_deadline()) {
                Work::Shutdown => break,
                Work::Settings(settings) => self.apply_settings(*settings),
                Work::Job(id, job) => {
                    self.run_job(id, &job);
                    self.shared.finish_running();
                }
                Work::Timeout => self.on_timer(),
            }
        }
        // Stop the watcher (joins its thread) before the store goes away.
        self.watcher = None;
    }

    fn send(&self, event: IndexEvent) {
        // The receiver may be gone while the handle is still alive; the index keeps working.
        let _ = self.events.send(event);
    }

    fn startup(&mut self) {
        match discover_volumes(&self.settings.volumes) {
            Ok(found) => {
                if let Err(e) = self.store.sync_volumes(&found) {
                    tracing::warn!(error = %e, "cannot record discovered volumes");
                }
            }
            Err(e) => tracing::warn!(error = %e, "volume discovery failed"),
        }
        self.publish();
        let volumes = self.store.volumes().unwrap_or_default();
        self.volumes_sent.clone_from(&volumes);
        self.send(IndexEvent::VolumesChanged(volumes));
        self.restart_watcher();
    }

    /// Loads a fresh snapshot, makes it current and announces it.
    fn publish(&mut self) {
        let started = Instant::now();
        match self.store.load_snapshot() {
            Ok(snapshot) => {
                let snapshot = Arc::new(snapshot);
                self.shared.set_snapshot(Arc::clone(&snapshot));
                self.last_load = started.elapsed();
                self.last_publish = Instant::now();
                self.dirty = false;
                tracing::debug!(
                    entries = snapshot.len(),
                    ms = self.last_load.as_millis(),
                    "snapshot published"
                );
                self.send(IndexEvent::SnapshotReady(snapshot));
            }
            Err(e) => tracing::error!(error = %e, "cannot load the index snapshot"),
        }
    }

    fn publish_interval(&self) -> Duration {
        MIN_PUBLISH_INTERVAL.max(self.last_load * PUBLISH_LOAD_FACTOR)
    }

    /// Publishes if data changed. Live-update jobs (`force == false`) are rate limited and wait
    /// for the queue to drain, because a reload costs seconds on large indexes.
    fn publish_if_due(&mut self, force: bool) {
        if !self.dirty || self.shared.is_shutdown() {
            return;
        }
        if !force && (self.shared.has_queued() || self.last_publish.elapsed() < self.publish_interval()) {
            return;
        }
        self.publish();
        self.announce_volumes(false);
    }

    fn announce_volumes(&mut self, force: bool) {
        if let Ok(volumes) = self.store.volumes()
            && (force || volumes != self.volumes_sent)
        {
            self.volumes_sent.clone_from(&volumes);
            self.send(IndexEvent::VolumesChanged(volumes));
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let publish = self.dirty.then(|| self.last_publish + self.publish_interval());
        let minutes = self.settings.indexer.effective_scheduled_rescan_minutes();
        let scheduled =
            (minutes > 0).then(|| self.last_scheduled + Duration::from_secs(u64::from(minutes) * 60));
        [publish, scheduled].into_iter().flatten().min()
    }

    fn on_timer(&mut self) {
        self.publish_if_due(false);
        let minutes = self.settings.indexer.effective_scheduled_rescan_minutes();
        if minutes > 0 && self.last_scheduled.elapsed() >= Duration::from_secs(u64::from(minutes) * 60) {
            self.last_scheduled = Instant::now();
            self.shared.submit(Job::RescanAll(ScanMode::Quick));
        }
    }

    fn apply_settings(&mut self, new: IndexerSettings) {
        let old = std::mem::replace(&mut self.settings, new);
        self.exclusions = compile_lenient(&self.settings.exclusions);
        if old.indexer.threads != self.settings.indexer.threads {
            self.pool = make_pool(self.settings.indexer.threads);
        }
        if old.indexer.effective_scheduled_rescan_minutes()
            != self.settings.indexer.effective_scheduled_rescan_minutes()
        {
            self.last_scheduled = Instant::now();
        }
        let watch_changed = old.indexer.effective_live_updates()
            != self.settings.indexer.effective_live_updates()
            || old.indexer.watch_roots != self.settings.indexer.watch_roots
            || old.exclusions != self.settings.exclusions
            || old.volumes != self.settings.volumes;
        if watch_changed {
            self.restart_watcher();
        }
    }

    fn restart_watcher(&mut self) {
        self.watcher = None;
        if !self.settings.indexer.effective_live_updates() {
            return;
        }
        let volumes = self.store.volumes().unwrap_or_default();
        let roots = effective_roots(&self.settings.indexer, &volumes);
        if roots.is_empty() {
            return;
        }
        let shared = Arc::clone(&self.shared);
        let mut config = WatchConfig::new(roots, self.exclusions.clone());
        // Never watch the index's own folder or the app's config/log folders: their writes would
        // feed back as change events.
        config.ignore = self
            .store
            .path()
            .parent()
            .map(std::path::Path::to_path_buf)
            .into_iter()
            .chain(self.settings.watch_ignore.iter().cloned())
            .collect();
        match Watcher::start(
            config,
            Arc::new(move |job| {
                shared.submit(job);
            }),
        ) {
            Ok(watcher) => self.watcher = Some(watcher),
            Err(e) => tracing::warn!(error = %e, "live updates unavailable"),
        }
    }

    fn run_job(&mut self, id: super::JobId, job: &Job) {
        let started = Instant::now();
        self.send(IndexEvent::Started {
            job: id,
            description: describe(job),
        });
        let expected_entries = if matches!(job, Job::FullReindex) {
            self.store.stats().ok().map(|s| s.entries)
        } else {
            None
        };
        let mut tracker = Tracker::new();
        let result = {
            let ctx = Ctx {
                job: id,
                events: &self.events,
                shared: &self.shared,
                settings: &self.settings,
                exclusions: &self.exclusions,
                pool: self.pool.as_ref(),
                expected_entries,
            };
            execute(&ctx, &mut self.store, &mut tracker, job)
        };
        if tracker.announced_scanning {
            // Undo the transient "scanning" state shown while the job ran.
            self.announce_volumes(true);
        }
        // Not throttled for a rename either: the user just acted and expects to see the new name.
        let user_initiated = !matches!(job, Job::RefreshDirs(_));
        match result {
            Ok(out) => {
                self.dirty |= out.changed;
                self.publish_if_due(user_initiated);
                self.send(IndexEvent::Finished {
                    job: id,
                    stats: JobStats {
                        upserted: out.upserted,
                        removed: out.removed,
                        dirs_listed: out.dirs_listed,
                        errors: out.errors,
                        elapsed: started.elapsed(),
                    },
                });
            }
            Err(IndexError::Cancelled) => {
                // Volumes finished before the cancel stay committed.
                self.dirty |= !matches!(job, Job::FullReindex);
                self.publish_if_due(true);
                self.send(IndexEvent::Cancelled { job: id });
            }
            Err(e) => {
                tracing::error!(job = id.0, error = %e, "job failed");
                self.dirty |= !matches!(job, Job::FullReindex);
                self.publish_if_due(true);
                self.send(IndexEvent::Failed {
                    job: id,
                    error: error_chain(&e),
                });
            }
        }
    }
}

fn execute(
    ctx: &Ctx<'_>,
    store: &mut Store,
    tracker: &mut Tracker,
    job: &Job,
) -> Result<Outcome, IndexError> {
    match job {
        Job::FullReindex => jobs::full_reindex(ctx, store, tracker),
        Job::RescanAll(mode) => jobs::rescan_all(ctx, store, tracker, *mode),
        Job::RescanVolume { volume, mode } => jobs::rescan_volume(ctx, store, tracker, volume, *mode),
        Job::RescanPath { path, mode } => jobs::rescan_path(ctx, store, tracker, path, *mode),
        Job::ApplyExclusions => jobs::apply_exclusions(ctx, store),
        Job::RefreshDirs(paths) => jobs::refresh_dirs(ctx, store, tracker, paths),
        Job::Renamed { from, to } => jobs::renamed(ctx, store, tracker, from, to),
    }
}

#[cfg(test)]
mod tests {
    use everythux_core::exclusion::RuleKind;

    use super::*;

    #[test]
    fn invalid_rules_are_skipped_not_fatal() {
        let set = compile_lenient(&[
            ExclusionRule::new("[", RuleKind::Any),
            ExclusionRule::new("node_modules", RuleKind::Dir),
            ExclusionRule::new("re:(", RuleKind::Any),
        ]);
        assert!(set.is_excluded(std::path::Path::new("/a/node_modules"), true));
        assert!(!set.is_excluded(std::path::Path::new("/a/other"), true));
    }

    fn idle_worker(dir: &std::path::Path, minutes: u32) -> Worker {
        let now = Instant::now();
        let mut settings = IndexerSettings::default();
        settings.indexer.scheduled_rescan_minutes = minutes;
        Worker {
            shared: Arc::new(Shared::new()),
            store: Store::open(&dir.join("index.db")).expect("open"),
            events: crossbeam_channel::unbounded().0,
            settings,
            exclusions: ExclusionSet::empty(),
            pool: None,
            watcher: None,
            volumes_sent: Vec::new(),
            dirty: false,
            last_publish: now,
            last_load: Duration::from_secs(1),
            last_scheduled: now,
        }
    }

    #[test]
    fn wake_ups_follow_the_schedule_and_pending_snapshots() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut worker = idle_worker(dir.path(), 0);
        assert_eq!(
            worker.next_deadline(),
            None,
            "an idle indexer never wakes up by itself"
        );

        worker.dirty = true;
        let publish = worker.last_publish + Duration::from_secs(4);
        assert_eq!(
            worker.next_deadline(),
            Some(publish),
            "4x the last reload time, at least 200 ms"
        );

        worker.settings.indexer.scheduled_rescan_minutes = 1;
        let scheduled = worker.last_scheduled + Duration::from_secs(60);
        assert_eq!(worker.next_deadline(), Some(publish), "the earlier of the two");
        worker.dirty = false;
        assert_eq!(worker.next_deadline(), Some(scheduled));
    }

    #[test]
    fn the_timer_queues_a_quick_rescan_when_due() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut worker = idle_worker(dir.path(), 1);
        worker.last_scheduled = Instant::now()
            .checked_sub(Duration::from_secs(61))
            .expect("monotonic clock far enough from its origin");
        worker.on_timer();
        match worker.shared.next_work(Some(Instant::now())) {
            Work::Job(_, job) => assert_eq!(job, Job::RescanAll(ScanMode::Quick)),
            other => panic!("expected the scheduled rescan, got {other:?}"),
        }
        assert!(
            worker.last_scheduled.elapsed() < Duration::from_secs(5),
            "the schedule restarts"
        );
    }

    #[test]
    fn manual_mode_has_no_timer_wake_ups_and_queues_no_scheduled_rescan() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut worker = idle_worker(dir.path(), 1);
        worker.settings.indexer.automatic = false;
        assert_eq!(worker.next_deadline(), None, "the schedule is ignored");
        worker.last_scheduled = Instant::now()
            .checked_sub(Duration::from_secs(3600))
            .expect("monotonic clock far enough from its origin");
        worker.on_timer();
        assert!(
            matches!(worker.shared.next_work(Some(Instant::now())), Work::Timeout),
            "nothing is queued by the timer"
        );
        // Pending snapshots still wake the worker: they are the result of a user action.
        worker.dirty = true;
        assert!(worker.next_deadline().is_some());
    }

    #[test]
    fn manual_mode_never_starts_the_watcher() {
        let (db, tree) = (
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
        );
        let mut worker = idle_worker(db.path(), 0);
        worker.settings.indexer.live_updates = true;
        worker.settings.indexer.watch_roots = vec![tree.path().to_path_buf()];
        worker.settings.volumes.include = vec![tree.path().to_path_buf()];
        worker.settings.volumes.auto_discover_fs_types = Vec::new();
        worker.startup();
        assert!(worker.watcher.is_some(), "control: automatic mode watches");

        let mut manual = worker.settings.clone();
        manual.indexer.automatic = false;
        worker.apply_settings(manual.clone());
        assert!(worker.watcher.is_none(), "switching to manual stops the watcher");
        worker.restart_watcher();
        assert!(worker.watcher.is_none(), "and nothing restarts it");

        manual.indexer.automatic = true;
        worker.apply_settings(manual);
        assert!(worker.watcher.is_some(), "switching back starts it again");
    }

    #[test]
    fn error_chain_lists_causes() {
        let e = IndexError::io("/x", std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        let text = error_chain(&e);
        assert!(
            text.contains("/x") && text.contains("permission denied"),
            "{text}"
        );
    }
}

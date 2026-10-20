//! Worker-thread plumbing: generic task workers, event forwarders and the debounced config writer.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use everythux_core::config::Config;
use everythux_core::dirty::DirtyFolders;
use everythux_core::history::SearchHistory;
use everythux_index::IndexEvent;

use super::{BackendEvent, Compute, SnapshotSlot};

/// How long the config writer waits for further changes before touching the disk.
pub(super) const CONFIG_DEBOUNCE: Duration = Duration::from_millis(500);

/// Spawns a thread that runs `handle` for every task and forwards the produced events to the UI.
pub(super) fn spawn_worker<T, F>(
    name: &str,
    events: Sender<BackendEvent>,
    ctx: egui::Context,
    mut handle: F,
) -> Sender<T>
where
    T: Send + 'static,
    F: FnMut(T) -> Vec<BackendEvent> + Send + 'static,
{
    let (tx, rx) = crossbeam_channel::unbounded::<T>();
    let spawned = std::thread::Builder::new().name(name.to_owned()).spawn(move || {
        for task in rx {
            for event in handle(task) {
                if events.send(event).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        }
    });
    if let Err(e) = spawned {
        tracing::error!(worker = name, error = %e, "cannot spawn worker thread");
    }
    tx
}

/// Spawns the compute worker. Previews queued behind a newer preview are dropped, because only
/// the latest exclusion popup state is of interest.
pub(super) fn spawn_compute_worker(events: Sender<BackendEvent>, ctx: egui::Context) -> Sender<Compute> {
    let (tx, rx) = crossbeam_channel::unbounded::<Compute>();
    let spawned = std::thread::Builder::new()
        .name("everythux-compute".to_owned())
        .spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut batch = vec![first];
                batch.extend(rx.try_iter());
                for task in coalesce(batch) {
                    if events.send(crate::tasks::run(task)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            }
        });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "cannot spawn compute worker");
    }
    tx
}

/// Drops every exclusion preview except the last one; keeps the order of everything else.
pub(super) fn coalesce(tasks: Vec<Compute>) -> Vec<Compute> {
    let last_preview = tasks
        .iter()
        .rposition(|t| matches!(t, Compute::ExclusionPreview { .. }));
    tasks
        .into_iter()
        .enumerate()
        .filter(|(i, t)| !matches!(t, Compute::ExclusionPreview { .. }) || Some(*i) == last_preview)
        .map(|(_, t)| t)
        .collect()
}

/// Forwards a service's result channel into the UI event channel until either side closes.
pub(super) fn forward<T, F>(rx: Receiver<T>, events: Sender<BackendEvent>, ctx: egui::Context, wrap: F)
where
    T: Send + 'static,
    F: Fn(T) -> BackendEvent + Send + 'static,
{
    let spawned = std::thread::Builder::new()
        .name("everythux-forward".to_owned())
        .spawn(move || {
            for item in rx {
                if events.send(wrap(item)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "cannot spawn forwarder thread");
    }
}

/// Forwards the indexer's events into the UI event channel like [`forward`], except that snapshots
/// go through a [`SnapshotSlot`]: however long the UI does not drain the channel (a minimized
/// window), at most one snapshot waits for it.
pub(super) fn forward_index(rx: Receiver<IndexEvent>, events: Sender<BackendEvent>, ctx: egui::Context) {
    let spawned = std::thread::Builder::new()
        .name("everythux-forward-index".to_owned())
        .spawn(move || relay_index(&rx, &events, &ctx));
    if let Err(e) = spawned {
        tracing::error!(error = %e, "cannot spawn the index event forwarder");
    }
}

/// Body of [`forward_index`]; returns when either channel closes.
fn relay_index(rx: &Receiver<IndexEvent>, events: &Sender<BackendEvent>, ctx: &egui::Context) {
    let slot = Arc::new(SnapshotSlot::default());
    for event in rx {
        let event = match event {
            IndexEvent::SnapshotReady(snapshot) => {
                if !slot.put(snapshot) {
                    // The UI has not taken the previous snapshot yet; the event that tells it to
                    // is still queued and will now find this newer one.
                    continue;
                }
                BackendEvent::Snapshot(Arc::clone(&slot))
            }
            other => BackendEvent::Index(other),
        };
        if events.send(event).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// Hands values to a background thread that writes only the newest one after a quiet period of
/// `debounce`. Dropping it flushes the pending value.
#[derive(Debug)]
struct Debounced<T> {
    tx: Option<Sender<T>>,
    thread: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> Debounced<T> {
    fn start(name: &str, debounce: Duration, mut write: impl FnMut(&T) + Send + 'static) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded::<T>();
        let thread = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                while let Ok(mut value) = rx.recv() {
                    loop {
                        match rx.recv_timeout(debounce) {
                            Ok(newer) => value = newer,
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => {
                                write(&value);
                                return;
                            }
                        }
                    }
                    write(&value);
                }
            })
            .map_err(|e| tracing::error!(error = %e, name, "cannot spawn writer thread"))
            .ok();
        Self { tx: Some(tx), thread }
    }

    fn send(&self, value: T) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(value);
        }
    }
}

impl<T> Drop for Debounced<T> {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Writes the config on a background thread, at most once per quiet period of
/// [`CONFIG_DEBOUNCE`]. Dropping it flushes the pending write.
#[derive(Debug)]
pub(super) struct ConfigSaver(Debounced<Config>);

impl ConfigSaver {
    pub(super) fn start(
        path: PathBuf,
        debounce: Duration,
        events: Sender<BackendEvent>,
        ctx: egui::Context,
    ) -> Self {
        Self(Debounced::start(
            "everythux-config-writer",
            debounce,
            move |config: &Config| {
                if let Err(e) = config.save(&path) {
                    tracing::error!(error = %e, "cannot save config");
                    let _ = events.send(BackendEvent::ConfigSaveFailed(e.to_string()));
                    ctx.request_repaint();
                }
            },
        ))
    }

    pub(super) fn save(&self, config: Config) {
        self.0.send(config);
    }
}

/// Writes the search history the same way as [`ConfigSaver`] writes the config.
#[derive(Debug)]
pub(super) struct HistorySaver(Debounced<SearchHistory>);

impl HistorySaver {
    pub(super) fn start(path: PathBuf, debounce: Duration) -> Self {
        Self(Debounced::start(
            "everythux-history-writer",
            debounce,
            move |history: &SearchHistory| {
                if let Err(e) = history.save(&path) {
                    tracing::error!(error = %e, "cannot save the search history");
                }
            },
        ))
    }

    pub(super) fn save(&self, history: SearchHistory) {
        self.0.send(history);
    }
}

/// Writes the out-of-sync folders the same way as [`ConfigSaver`] writes the config.
#[derive(Debug)]
pub(super) struct DirtySaver(Debounced<DirtyFolders>);

impl DirtySaver {
    pub(super) fn start(path: PathBuf, debounce: Duration) -> Self {
        Self(Debounced::start(
            "everythux-dirty-writer",
            debounce,
            move |dirty: &DirtyFolders| {
                if let Err(e) = dirty.save(&path) {
                    tracing::error!(error = %e, "cannot save the out-of-sync folders");
                }
            },
        ))
    }

    pub(super) fn save(&self, dirty: DirtyFolders) {
        self.0.send(dirty);
    }
}

#[cfg(test)]
mod tests {
    use everythux_core::exclusion::{ExclusionRule, RuleKind};
    use everythux_core::snapshot::IndexSnapshot;
    use everythux_index::{JobId, JobStats};

    use super::*;

    /// Regression test: a minimized window drains nothing while live updates keep publishing
    /// snapshots. Every queued snapshot used to stay alive (tens of GB after an idle hour).
    #[test]
    fn undrained_snapshots_do_not_pile_up() {
        let (index_tx, index_rx) = crossbeam_channel::unbounded();
        let (events, ui) = crossbeam_channel::unbounded();
        let mut published = Vec::new();
        for job in 1..=50 {
            let snapshot = Arc::new(IndexSnapshot::empty());
            published.push(Arc::downgrade(&snapshot));
            index_tx.send(IndexEvent::SnapshotReady(snapshot)).expect("open");
            index_tx
                .send(IndexEvent::Finished {
                    job: JobId(job),
                    stats: JobStats::default(),
                })
                .expect("open");
        }
        drop(index_tx);
        relay_index(&index_rx, &events, &egui::Context::default());

        let alive = published.iter().filter(|s| s.strong_count() > 0).count();
        assert_eq!(alive, 1, "only the newest snapshot is kept for the UI");

        let queued: Vec<BackendEvent> = ui.try_iter().collect();
        let finished = queued
            .iter()
            .filter(|e| matches!(e, BackendEvent::Index(IndexEvent::Finished { .. })))
            .count();
        assert_eq!(finished, 50, "other index events are all delivered");
        let wake_ups: Vec<&Arc<SnapshotSlot>> = queued
            .iter()
            .filter_map(|e| match e {
                BackendEvent::Snapshot(slot) => Some(slot),
                _ => None,
            })
            .collect();
        assert_eq!(wake_ups.len(), 1, "one wake-up for the UI");
        let newest = wake_ups[0].take().expect("the newest snapshot waits in the slot");
        let last = published.last().expect("published");
        assert!(std::ptr::eq(Arc::as_ptr(&newest), last.as_ptr()));
    }

    #[test]
    fn snapshot_slot_wakes_the_ui_only_when_it_was_empty() {
        let slot = SnapshotSlot::default();
        let first = Arc::new(IndexSnapshot::empty());
        let second = Arc::new(IndexSnapshot::empty());
        assert!(slot.put(Arc::clone(&first)), "empty slot: queue a wake-up");
        assert!(!slot.put(Arc::clone(&second)), "the wake-up is still queued");
        assert_eq!(Arc::strong_count(&first), 1, "the replaced snapshot is dropped");
        let taken = slot.take().expect("waiting");
        assert!(Arc::ptr_eq(&taken, &second));
        assert!(slot.take().is_none());
        assert!(slot.put(first), "taken: the next snapshot needs a new wake-up");
    }

    fn preview(request: u64) -> Compute {
        Compute::ExclusionPreview {
            request,
            snapshot: Arc::new(IndexSnapshot::empty()),
            rules: vec![ExclusionRule::new("x", RuleKind::Dir)],
        }
    }

    #[test]
    fn coalesce_keeps_only_the_latest_preview() {
        let tasks = vec![
            preview(1),
            Compute::ImportRules("a.json".into()),
            preview(2),
            preview(3),
        ];
        let kept = coalesce(tasks);
        assert_eq!(kept.len(), 2);
        assert!(matches!(kept[0], Compute::ImportRules(_)));
        assert!(matches!(kept[1], Compute::ExclusionPreview { request: 3, .. }));
    }

    #[test]
    fn saver_debounces_and_flushes_on_drop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.json");
        let (events, errors) = crossbeam_channel::unbounded();
        let saver = ConfigSaver::start(
            path.clone(),
            Duration::from_secs(30),
            events,
            egui::Context::default(),
        );
        let mut config = Config::default();
        for width in [100.0, 200.0, 300.0] {
            config.ui.preview_pane.width = width;
            saver.save(config.clone());
        }
        assert!(!path.exists(), "nothing is written during the quiet period");
        drop(saver);
        let loaded = Config::load(&path).expect("flushed on drop");
        assert!(
            (loaded.ui.preview_pane.width - 300.0).abs() < f32::EPSILON,
            "latest wins"
        );
        assert!(errors.try_recv().is_err(), "no save errors");
    }

    #[test]
    fn saver_reports_write_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        // The "file" is an existing directory, so the rename fails.
        let path = dir.path().to_owned();
        let (events, errors) = crossbeam_channel::unbounded();
        let saver = ConfigSaver::start(path, Duration::from_millis(1), events, egui::Context::default());
        saver.save(Config::default());
        drop(saver);
        assert!(matches!(errors.try_recv(), Ok(BackendEvent::ConfigSaveFailed(_))));
    }
}

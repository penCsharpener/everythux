//! Worker-thread plumbing: generic task workers, event forwarders and the debounced config writer.

use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use everythux_core::config::Config;
use everythux_core::history::SearchHistory;

use super::{BackendEvent, Compute};

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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use everythux_core::exclusion::{ExclusionRule, RuleKind};
    use everythux_core::snapshot::IndexSnapshot;

    use super::*;

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

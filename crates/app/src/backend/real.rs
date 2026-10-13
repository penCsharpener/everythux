//! The production [`Backend`]: real indexer, search service, preview service and workers.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use crossbeam_channel::{Receiver, Sender};
use everythux_core::config::Config;
use everythux_core::paths::AppPaths;
use everythux_core::snapshot::IndexSnapshot;
use everythux_index::{Indexer, IndexerSettings, Job};
use everythux_platform::file_manager;
use everythux_platform::mime::{self, Category};
use everythux_platform::udisks;
use everythux_preview::{PreviewLimits, PreviewService};
use everythux_search::{SearchRequest, SearchService};

use super::workers::{CONFIG_DEBOUNCE, ConfigSaver, forward, spawn_compute_worker, spawn_worker};
use super::{Backend, BackendEvent, Compute, PlatformAction};

enum UdisksTask {
    List(Vec<String>),
    Mount(String),
}

struct PreviewHandle {
    service: PreviewService,
    limits: PreviewLimits,
}

/// Runs the real services. Every call just queues work; nothing here blocks the UI thread.
pub(crate) struct RealBackend {
    indexer: Option<Indexer>,
    search: SearchService,
    preview: Mutex<Option<PreviewHandle>>,
    events: Sender<BackendEvent>,
    ctx: egui::Context,
    platform_tx: Sender<PlatformAction>,
    udisks_tx: Sender<UdisksTask>,
    compute_tx: Sender<Compute>,
    saver: ConfigSaver,
}

impl RealBackend {
    /// Starts all services. A failing indexer (e.g. a corrupt database) is reported through a
    /// [`BackendEvent::Fatal`] event so the UI can still come up and explain what happened.
    pub(crate) fn start(
        ctx: &egui::Context,
        paths: &AppPaths,
        settings: IndexerSettings,
    ) -> (Self, Receiver<BackendEvent>) {
        let (events, rx) = crossbeam_channel::unbounded();

        let indexer = match Indexer::start(&paths.db_file, settings) {
            Ok((indexer, index_rx)) => {
                forward(index_rx, events.clone(), ctx.clone(), BackendEvent::Index);
                Some(indexer)
            }
            Err(e) => {
                tracing::error!(error = %e, db = %paths.db_file.display(), "cannot start indexer");
                let _ = events.send(BackendEvent::Fatal(format!(
                    "Cannot open the index database {}: {e}",
                    paths.db_file.display()
                )));
                None
            }
        };

        let (search, search_rx) = SearchService::start(Arc::new(IndexSnapshot::empty()));
        forward(search_rx, events.clone(), ctx.clone(), BackendEvent::Search);

        let platform_tx = spawn_worker(
            "everythux-platform",
            events.clone(),
            ctx.clone(),
            |action: PlatformAction| {
                let result = match &action {
                    PlatformAction::OpenDefault(p) => file_manager::open_default(p),
                    PlatformAction::ShowItem(p) => file_manager::show_item(p),
                    PlatformAction::ShowProperties(t) => file_manager::show_properties(&t.path),
                };
                vec![BackendEvent::Platform { action, result }]
            },
        );
        // UDisks2 calls (mounting may wait for a polkit prompt) get their own thread so that they
        // never delay opening files.
        let udisks_tx = spawn_worker(
            "everythux-udisks",
            events.clone(),
            ctx.clone(),
            |task: UdisksTask| {
                vec![match task {
                    UdisksTask::List(types) => {
                        BackendEvent::Devices(udisks::list_filesystems(&types).map_err(|e| e.to_string()))
                    }
                    UdisksTask::Mount(object_path) => {
                        let result = udisks::mount(&object_path).map_err(|e| e.to_string());
                        BackendEvent::Mounted { object_path, result }
                    }
                }]
            },
        );
        let compute_tx = spawn_compute_worker(events.clone(), ctx.clone());
        let saver = ConfigSaver::start(
            paths.config_file.clone(),
            CONFIG_DEBOUNCE,
            events.clone(),
            ctx.clone(),
        );

        (
            Self {
                indexer,
                search,
                preview: Mutex::new(None),
                events,
                ctx: ctx.clone(),
                platform_tx,
                udisks_tx,
                compute_tx,
                saver,
            },
            rx,
        )
    }
}

impl Backend for RealBackend {
    fn submit_job(&self, job: Job) {
        if let Some(indexer) = &self.indexer {
            let id = indexer.submit(job);
            tracing::debug!(?id, "index job submitted");
        }
    }

    fn cancel_job(&self) {
        if let Some(indexer) = &self.indexer {
            indexer.cancel_current();
        }
    }

    fn update_settings(&self, settings: IndexerSettings) {
        if let Some(indexer) = &self.indexer {
            indexer.update_settings(settings);
        }
    }

    fn search(&self, request: SearchRequest) -> u64 {
        self.search.submit(request)
    }

    fn set_snapshot(&self, snapshot: Arc<IndexSnapshot>) {
        self.search.set_snapshot(snapshot);
    }

    fn request_preview(&self, path: PathBuf, limits: PreviewLimits) -> u64 {
        let mut guard = self.preview.lock().unwrap_or_else(PoisonError::into_inner);
        let handle = guard.get_or_insert_with(|| {
            let (service, rx) = PreviewService::start(limits);
            forward(rx, self.events.clone(), self.ctx.clone(), BackendEvent::Preview);
            PreviewHandle { service, limits }
        });
        if handle.limits != limits {
            handle.service.set_limits(limits);
            handle.limits = limits;
        }
        handle.service.request(path)
    }

    fn platform(&self, action: PlatformAction) {
        let _ = self.platform_tx.send(action);
    }

    fn list_devices(&self, fs_types: Vec<String>) {
        let _ = self.udisks_tx.send(UdisksTask::List(fs_types));
    }

    fn mount(&self, object_path: String) {
        let _ = self.udisks_tx.send(UdisksTask::Mount(object_path));
    }

    fn compute(&self, task: Compute) {
        let _ = self.compute_tx.send(task);
    }

    fn save_config(&self, config: &Config) {
        self.saver.save(config.clone());
    }

    fn describe_extension(&self, ext: &str) -> Arc<str> {
        mime::describe_extension(ext)
    }

    fn category(&self, ext: &str) -> Category {
        mime::category(ext)
    }
}

//! Test double for [`Backend`]: records every call and lets tests inject events.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use everythux_core::config::Config;
use everythux_core::ids::EntryId;
use everythux_core::snapshot::IndexSnapshot;
use everythux_index::{IndexerSettings, Job};
use everythux_platform::PlatformError;
use everythux_platform::file_manager::Method;
use everythux_platform::mime::Category;
use everythux_preview::{PreviewLimits, TextEncoding};
use everythux_search::{SearchError, SearchRequest, SearchResults};

use super::{Backend, BackendEvent, Compute, PlatformAction};

/// One recorded backend call.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Call {
    SubmitJob(Job),
    CancelJob,
    UpdateSettings(Box<IndexerSettings>),
    Search(SearchRequest),
    SetSnapshot,
    RequestPreview(PathBuf),
    RequestTextRange {
        path: PathBuf,
        offset: u64,
        len: u64,
        encoding: TextEncoding,
    },
    Platform(PlatformAction),
    LoadActions,
    ListDevices(Vec<String>),
    Mount(String),
    Compute(&'static str),
    SaveConfig(Box<Config>),
}

#[derive(Debug, Default)]
struct State {
    calls: Vec<Call>,
    generation: u64,
    preview_request: u64,
    properties_unsupported: bool,
}

/// The backend handed to the app under test.
#[derive(Debug)]
pub(crate) struct FakeBackend {
    state: Arc<Mutex<State>>,
    tx: Sender<BackendEvent>,
}

/// The test's view of the fake: inspect calls, inject events.
#[derive(Debug, Clone)]
pub(crate) struct FakeHandle {
    state: Arc<Mutex<State>>,
    tx: Sender<BackendEvent>,
}

impl FakeBackend {
    pub(crate) fn new() -> (Self, FakeHandle, Receiver<BackendEvent>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let state = Arc::new(Mutex::new(State::default()));
        (
            Self {
                state: Arc::clone(&state),
                tx: tx.clone(),
            },
            FakeHandle { state, tx },
            rx,
        )
    }

    fn record(&self, call: Call) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .calls
            .push(call);
    }
}

impl FakeHandle {
    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// All calls so far.
    pub(crate) fn calls(&self) -> Vec<Call> {
        self.with(|s| s.calls.clone())
    }

    /// Forgets the recorded calls.
    pub(crate) fn clear(&self) {
        self.with(|s| s.calls.clear());
    }

    /// Recorded search requests, oldest first.
    pub(crate) fn searches(&self) -> Vec<SearchRequest> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::Search(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    /// Recorded platform actions, oldest first.
    pub(crate) fn platform_actions(&self) -> Vec<PlatformAction> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::Platform(a) => Some(a),
                _ => None,
            })
            .collect()
    }

    /// Recorded index jobs, oldest first.
    pub(crate) fn jobs(&self) -> Vec<Job> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::SubmitJob(j) => Some(j),
                _ => None,
            })
            .collect()
    }

    /// The most recent config passed to `save_config`.
    pub(crate) fn last_saved_config(&self) -> Option<Config> {
        self.calls().into_iter().rev().find_map(|c| match c {
            Call::SaveConfig(c) => Some(*c),
            _ => None,
        })
    }

    /// The most recent indexer settings.
    pub(crate) fn last_settings(&self) -> Option<IndexerSettings> {
        self.calls().into_iter().rev().find_map(|c| match c {
            Call::UpdateSettings(s) => Some(*s),
            _ => None,
        })
    }

    /// Make `ShowProperties` fail with `Unsupported`, like a desktop without the D-Bus method.
    pub(crate) fn set_properties_unsupported(&self, unsupported: bool) {
        self.with(|s| s.properties_unsupported = unsupported);
    }

    /// Injects an event as if a worker had produced it.
    pub(crate) fn send(&self, event: BackendEvent) {
        let _ = self.tx.send(event);
    }

    /// Injects search results for `generation`; a parse `error` is attributed to the first bar.
    pub(crate) fn send_results(
        &self,
        generation: u64,
        snapshot: &Arc<IndexSnapshot>,
        ids: &[EntryId],
        error: Option<&str>,
    ) {
        let error = error.map(|message| SearchError {
            bar: 0,
            message: message.to_owned(),
        });
        self.send_results_with(generation, snapshot, ids, error);
    }

    /// Injects search results for `generation` with an optional per-bar parse error.
    pub(crate) fn send_results_with(
        &self,
        generation: u64,
        snapshot: &Arc<IndexSnapshot>,
        ids: &[EntryId],
        error: Option<SearchError>,
    ) {
        self.send(BackendEvent::Search(SearchResults {
            generation,
            ids: ids.into(),
            snapshot: Arc::clone(snapshot),
            total_entries: snapshot.len(),
            elapsed: Duration::from_millis(2),
            error,
        }));
    }
}

impl Backend for FakeBackend {
    fn submit_job(&self, job: Job) {
        self.record(Call::SubmitJob(job));
    }

    fn cancel_job(&self) {
        self.record(Call::CancelJob);
    }

    fn update_settings(&self, settings: IndexerSettings) {
        self.record(Call::UpdateSettings(Box::new(settings)));
    }

    fn search(&self, request: SearchRequest) -> u64 {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.generation += 1;
        s.calls.push(Call::Search(request));
        s.generation
    }

    fn set_snapshot(&self, _snapshot: Arc<IndexSnapshot>) {
        self.record(Call::SetSnapshot);
    }

    fn request_preview(&self, path: PathBuf, _limits: PreviewLimits) -> u64 {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.preview_request += 1;
        s.calls.push(Call::RequestPreview(path));
        s.preview_request
    }

    fn request_text_range(&self, path: PathBuf, offset: u64, len: u64, encoding: TextEncoding) -> u64 {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.preview_request += 1;
        s.calls.push(Call::RequestTextRange {
            path,
            offset,
            len,
            encoding,
        });
        s.preview_request
    }

    fn platform(&self, action: PlatformAction) {
        let unsupported = {
            let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            s.calls.push(Call::Platform(action.clone()));
            s.properties_unsupported
        };
        if matches!(action, PlatformAction::Rename { .. } | PlatformAction::Trash(_)) {
            // These answer with their own events, which the test injects.
            return;
        }
        let result = if unsupported && matches!(action, PlatformAction::ShowProperties(_)) {
            Err(PlatformError::Unsupported("no ShowItemProperties"))
        } else {
            Ok(match action {
                PlatformAction::Launch { .. } => Method::Launched,
                PlatformAction::CopyFiles(_) => Method::Clipboard,
                _ => Method::FileManager1,
            })
        };
        let _ = self.tx.send(BackendEvent::Platform { action, result });
    }

    fn load_actions(&self) {
        self.record(Call::LoadActions);
    }

    fn list_devices(&self, fs_types: Vec<String>) {
        self.record(Call::ListDevices(fs_types));
    }

    fn mount(&self, object_path: String) {
        self.record(Call::Mount(object_path));
    }

    fn compute(&self, task: Compute) {
        let kind = match &task {
            Compute::ExclusionPreview { .. } => "exclusion_preview",
            Compute::Properties { .. } => "properties",
            Compute::ImportRules(_) => "import_rules",
            Compute::ExportRules { .. } => "export_rules",
        };
        self.record(Call::Compute(kind));
        // The computations are pure or touch only test-owned paths, so run them inline.
        let _ = self.tx.send(crate::tasks::run(task));
    }

    fn save_config(&self, config: &Config) {
        self.record(Call::SaveConfig(Box::new(config.clone())));
    }

    fn describe_extension(&self, ext: &str) -> Arc<str> {
        if ext.is_empty() {
            Arc::from("File")
        } else {
            Arc::from(format!("{} file", ext.to_uppercase()))
        }
    }

    fn config_path(&self) -> PathBuf {
        "/home/me/.config/everythux/config.json".into()
    }

    fn category(&self, ext: &str) -> Category {
        match ext {
            "pdf" | "txt" => Category::Document,
            "png" | "jpg" => Category::Image,
            "rs" | "js" => Category::Code,
            _ => Category::Other,
        }
    }
}

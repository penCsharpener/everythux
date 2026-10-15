//! The boundary between the UI and everything that blocks: indexer, search, previews, desktop
//! integration, UDisks2, disk I/O.
//!
//! The UI only talks to the [`Backend`] trait. [`real::RealBackend`] runs the actual services on
//! worker threads; the test-only fake records calls and lets tests inject events. Results come back
//! as [`BackendEvent`]s on a single channel; workers wake the UI with `Context::request_repaint`.

pub(crate) mod real;
mod workers;

#[cfg(test)]
pub(crate) mod fake;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use everythux_core::config::{Bookmark, Config};
use everythux_core::exclusion::ExclusionRule;
use everythux_core::snapshot::IndexSnapshot;
use everythux_index::{IndexEvent, IndexerSettings, Job};
use everythux_platform::PlatformError;
use everythux_platform::actions::{ActionCatalog, Launcher};
use everythux_platform::file_manager::Method;
use everythux_platform::mime::Category;
use everythux_platform::udisks::BlockDevice;
use everythux_preview::{PreviewLimits, PreviewResult, TextEncoding};
use everythux_search::{SearchRequest, SearchResults};

use everythux_core::history::SearchHistory;

use crate::model::{PropTarget, PropertiesInfo};

/// Per source of a paste: its new path, or why it failed.
pub(crate) type PasteResult = (PathBuf, Result<PathBuf, String>);

/// Desktop actions executed on the platform worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlatformAction {
    /// Open with the default application (Name double-click, Enter).
    OpenDefault(PathBuf),
    /// Open the containing folder with the item selected (Path double-click).
    ShowItem(PathBuf),
    /// Ask the file manager for its properties dialog (Size / Modified double-click).
    ShowProperties(PropTarget),
    /// Start an "Open with" application or a service-menu action for `paths`.
    Launch { launcher: Launcher, paths: Vec<PathBuf> },
    /// Put these files on the clipboard as a file list (paste in a file manager).
    CopyFiles(Vec<PathBuf>),
    /// Put these files on the clipboard as a *cut* list: pasting moves them.
    CutFiles(Vec<PathBuf>),
    /// Paste the files on the clipboard into the folder `dest` (the outcome arrives as
    /// [`BackendEvent::Pasted`], not as [`BackendEvent::Platform`]).
    Paste { dest: PathBuf },
    /// Move these to the trash (the outcome arrives as [`BackendEvent::Trashed`], not as
    /// [`BackendEvent::Platform`]).
    Trash(Vec<PathBuf>),
    /// Rename `from` inside its folder (blocking `rename(2)`; the outcome arrives as
    /// [`BackendEvent::Renamed`], not as [`BackendEvent::Platform`]). `undo` marks a rename that
    /// reverts an earlier one.
    Rename {
        from: PathBuf,
        new_name: OsString,
        undo: bool,
    },
}

impl PlatformAction {
    /// Short description for log lines and the status bar.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::OpenDefault(p) => format!("Open {}", p.display()),
            Self::ShowItem(p) => format!("Show {} in folder", p.display()),
            Self::ShowProperties(t) => format!("Properties of {}", t.path.display()),
            Self::Launch { launcher, paths } => match paths.as_slice() {
                [one] => format!("Run {} on {}", launcher.name, one.display()),
                many => format!("Run {} on {} items", launcher.name, many.len()),
            },
            Self::CopyFiles(paths) => format!("Copy {} file(s) to the clipboard", paths.len()),
            Self::CutFiles(paths) => format!("Cut {} file(s) to the clipboard", paths.len()),
            Self::Paste { dest } => format!("Paste into {}", dest.display()),
            Self::Trash(paths) => format!("Move {} item(s) to the trash", paths.len()),
            Self::Rename { from, new_name, .. } => {
                format!("Rename {} to {}", from.display(), new_name.to_string_lossy())
            }
        }
    }
}

/// Work that only needs CPU or the local filesystem, run on the compute worker.
#[derive(Debug, Clone)]
pub(crate) enum Compute {
    /// Live preview of an exclusion popup / editor change: how much would `rules` remove?
    ExclusionPreview {
        request: u64,
        snapshot: Arc<IndexSnapshot>,
        rules: Vec<ExclusionRule>,
    },
    /// Counts and filesystem metadata for the built-in Properties window.
    Properties {
        id: u64,
        snapshot: Arc<IndexSnapshot>,
        target: PropTarget,
    },
    /// Reads exclusion rules from a JSON file.
    ImportRules(PathBuf),
    /// Writes exclusion rules to a JSON file.
    ExportRules {
        path: PathBuf,
        rules: Vec<ExclusionRule>,
    },
    /// Reads bookmarks from a JSON file.
    ImportBookmarks(PathBuf),
    /// Writes bookmarks to a JSON file.
    ExportBookmarks { path: PathBuf, bookmarks: Vec<Bookmark> },
}

/// Result of [`Compute::ExclusionPreview`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExclusionPreview {
    pub(crate) request: u64,
    /// Indexed entries the rules would remove.
    pub(crate) entries: u64,
    /// Folders affected.
    pub(crate) folders: u64,
    /// Compile error, if the rules are invalid.
    pub(crate) error: Option<String>,
}

/// Everything a worker can report back.
#[derive(Debug)]
pub(crate) enum BackendEvent {
    Index(IndexEvent),
    Search(SearchResults),
    Preview(PreviewResult),
    Platform {
        action: PlatformAction,
        result: Result<Method, PlatformError>,
    },
    /// Outcome of [`PlatformAction::Rename`]: the new absolute path, or the user-facing error.
    Renamed {
        from: PathBuf,
        new_name: OsString,
        undo: bool,
        result: Result<PathBuf, String>,
    },
    /// Outcome of [`PlatformAction::Paste`]: per source its new path or the reason it failed, or
    /// why the clipboard could not be read.
    Pasted {
        dest: PathBuf,
        cut: bool,
        results: Result<Vec<PasteResult>, String>,
    },
    /// Outcome of [`PlatformAction::Trash`], one result per path.
    Trashed(Vec<(PathBuf, Result<(), String>)>),
    /// The installed applications and file actions, read from the XDG directories.
    Actions(Arc<ActionCatalog>),
    Devices(Result<Vec<BlockDevice>, String>),
    Mounted {
        object_path: String,
        result: Result<PathBuf, String>,
    },
    ExclusionPreview(ExclusionPreview),
    Properties {
        id: u64,
        info: PropertiesInfo,
    },
    RulesImported(Result<Vec<ExclusionRule>, String>),
    RulesExported(Result<(PathBuf, usize), String>),
    BookmarksImported(Result<Vec<Bookmark>, String>),
    BookmarksExported(Result<(PathBuf, usize), String>),
    ConfigSaveFailed(String),
    /// A service could not start (e.g. the database is unusable); shown as a banner.
    Fatal(String),
}

/// All side effects the UI can request. Calls never block.
pub(crate) trait Backend {
    /// Queues an index job.
    fn submit_job(&self, job: Job);
    /// Cancels the running index job.
    fn cancel_job(&self);
    /// Pushes new volume/exclusion/indexer settings to the indexer.
    fn update_settings(&self, settings: IndexerSettings);
    /// Submits a search; returns its generation.
    fn search(&self, request: SearchRequest) -> u64;
    /// Hands the search service a new snapshot.
    fn set_snapshot(&self, snapshot: Arc<IndexSnapshot>);
    /// Requests a preview; returns the request id. Starts the preview service on first use.
    fn request_preview(&self, path: PathBuf, limits: PreviewLimits) -> u64;
    /// Requests `len` more bytes of text of `path` from `offset`, decoded as `encoding` ("Show
    /// more" under a text preview); the chunk arrives as [`BackendEvent::Preview`] with the
    /// returned request id. Like a preview request, it supersedes the one in flight.
    fn request_text_range(&self, path: PathBuf, offset: u64, len: u64, encoding: TextEncoding) -> u64;
    /// Runs a desktop action; the outcome arrives as [`BackendEvent::Platform`].
    fn platform(&self, action: PlatformAction);
    /// Reads the installed applications and file actions in the background; the catalog arrives as
    /// [`BackendEvent::Actions`].
    fn load_actions(&self);
    /// Asks UDisks2 for filesystems of these types; arrives as [`BackendEvent::Devices`].
    fn list_devices(&self, fs_types: Vec<String>);
    /// Mounts a device via UDisks2; arrives as [`BackendEvent::Mounted`].
    fn mount(&self, object_path: String);
    /// Runs a compute task; the result arrives as an event.
    fn compute(&self, task: Compute);
    /// Persists the config (debounced, atomic).
    fn save_config(&self, config: &Config);
    /// Human-readable type name for a lowercase extension (cached by the platform crate).
    fn describe_extension(&self, ext: &str) -> Arc<str>;
    /// Icon category for a lowercase extension.
    fn category(&self, ext: &str) -> Category;
    /// The JSON config file the app loads and saves.
    fn config_path(&self) -> PathBuf;
    /// Reads the saved search history (blocking; called once at startup). An unreadable file is
    /// an empty history.
    fn load_history(&self) -> SearchHistory;
    /// Saves the search history in the background.
    fn save_history(&self, history: &SearchHistory);
}

//! The application state machine.
//!
//! [`App`] owns all UI state. Everything that blocks goes through the [`Backend`]; results come back
//! as [`BackendEvent`]s which [`App::poll_events`] folds into the state at the start of each frame.
//! The egui rendering lives in `crate::ui`; the logic in the submodules here is free of widgets and
//! is exercised directly by the tests.

mod bookmarks;
mod commands;
mod context_menu;
mod events;
mod frame;
mod history;
mod interact;
mod rename;
mod search;
mod settings;
mod tabs;
mod windows;

use std::sync::Arc;

use crossbeam_channel::Receiver;
use egui::Pos2;
use everythux_core::config::Config;
use everythux_core::model::VolumeInfo;
use everythux_core::snapshot::IndexSnapshot;

pub(crate) use commands::Command;
pub(crate) use context_menu::ContextExtras;
pub(crate) use frame::bar_widget_id;
#[cfg(test)]
pub(crate) use frame::search_id;
pub(crate) use interact::{CellEvent, ContextAction, CopyKind, Effect};
pub(crate) use rename::CommitTrigger;
pub(crate) use settings::valid_rules;

/// Most tabs that can be open at once.
pub(crate) const fn tabs_limit() -> usize {
    tabs::MAX_TABS
}

use crate::backend::{Backend, BackendEvent};
use crate::exclusions::{ExclusionEditor, ExclusionPopup};
use crate::keys::Bindings;
use crate::model::ResultsView;
use crate::preview::PreviewState;
use crate::properties::PropertiesWindow;
use crate::rename::RenameEdit;
use crate::search_bars::SearchState;
use crate::selection::Selection;
use crate::status::{IndexStatus, Toast};
use crate::ui::table::RowCache;
use crate::volumes::VolumesWindow;
use tabs::TabSlot;

/// Row height of the result table in logical pixels.
pub(crate) const ROW_HEIGHT: f32 = 20.0;

/// Identifies a window or popup for the Escape-key stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PopupId {
    ConfirmReindex,
    ConfirmDelete,
    ExclusionPopup,
    ExclusionEditor,
    Volumes,
    Properties(u64),
    Shortcuts,
    About,
    PreviewWidth,
    Bookmarks,
}

/// The bookmarks window: whether it is open and the name typed for the next bookmark.
#[derive(Debug, Default)]
pub(crate) struct BookmarksWindow {
    pub(crate) open: bool,
    pub(crate) name: String,
    /// File to import from or export to.
    pub(crate) transfer_path: String,
    /// Result of the last import or export.
    pub(crate) message: Option<String>,
}

/// All secondary windows. `order` is the z-order, topmost last.
#[derive(Debug, Default)]
pub(crate) struct Windows {
    pub(crate) confirm_reindex: bool,
    /// Paths waiting for the user's confirmation to be deleted permanently.
    pub(crate) confirm_delete: Option<Vec<std::path::PathBuf>>,
    pub(crate) exclusion_popup: Option<ExclusionPopup>,
    pub(crate) exclusion_editor: ExclusionEditor,
    pub(crate) volumes: VolumesWindow,
    pub(crate) properties: Vec<PropertiesWindow>,
    pub(crate) shortcuts: bool,
    pub(crate) about: bool,
    pub(crate) bookmarks: BookmarksWindow,
    pub(crate) order: Vec<PopupId>,
    pub(crate) next_id: u64,
}

/// A warning banner at the top of the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Banner {
    pub(crate) text: String,
}

/// The whole application.
pub(crate) struct App {
    pub(crate) ctx: egui::Context,
    pub(crate) backend: Box<dyn Backend>,
    events: Receiver<BackendEvent>,
    pub(crate) config: Config,
    pub(crate) bindings: Bindings,
    pub(crate) banners: Vec<Banner>,

    /// The tabs other than the active one, in tab order; the slot of the active tab is unused.
    /// The active tab's own state is in `search`, `results` and `selection` below.
    pub(crate) tabs: Vec<TabSlot>,
    pub(crate) active_tab: usize,
    pub(crate) search: SearchState,
    pub(crate) results: Option<ResultsView>,
    pub(crate) selection: Selection,
    /// Row the table scrolls to on the next frame (keyboard navigation).
    pub(crate) scroll_to_row: Option<usize>,
    /// Rows that fit in the table viewport (for Page Up/Down); updated by the table.
    pub(crate) visible_rows: usize,
    pub(crate) cache: RowCache,
    /// Installed applications and file actions for the context menu (empty until loaded).
    pub(crate) actions: Arc<everythux_platform::actions::ActionCatalog>,
    /// `egui` time the catalog was last requested.
    pub(crate) actions_loaded_at: f64,
    /// Extra context-menu entries for the selection that was right-clicked last.
    pub(crate) context_extras: ContextExtras,

    /// Latest snapshot published by the indexer.
    pub(crate) snapshot: Arc<IndexSnapshot>,
    pub(crate) index: IndexStatus,
    pub(crate) volumes: Vec<VolumeInfo>,

    pub(crate) toasts: Vec<Toast>,
    pub(crate) windows: Windows,
    pub(crate) preview: PreviewState,

    /// The inline rename editor, if open.
    pub(crate) rename: Option<RenameEdit>,
    /// Path to select (and scroll to) once a snapshot containing it arrives.
    pub(crate) pending_reselect: Option<rename::PendingReselect>,
    /// Row selected by a plain click or key and when (`egui` time), for the slow-click rename.
    pub(crate) selected_since: Option<(usize, f64)>,
    /// A qualifying slow click waiting out the double-click interval: row and click time.
    pub(crate) slow_click: Option<(usize, f64)>,

    /// `egui` time of the current frame, seconds.
    pub(crate) now: f64,
    /// Latest pointer position, used to place popups near the cursor.
    pub(crate) pointer: Option<Pos2>,
    /// Set when the app asked the viewport to close.
    pub(crate) quit_requested: bool,
    /// Show only duplicate files (same name and size) among the matches. Not persisted: a list that
    /// silently starts filtered would be confusing.
    pub(crate) duplicates_only: bool,
    /// Every search ever used, remembered across sessions.
    pub(crate) history: everythux_core::history::SearchHistory,
    /// The history dropdown of the first search bar.
    pub(crate) dropdown: history::HistoryDropdown,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("search", &self.search)
            .field("rows", &self.results.as_ref().map(ResultsView::len))
            .finish_non_exhaustive()
    }
}

impl App {
    /// Creates the app, applies the theme and submits the initial (empty) search.
    pub(crate) fn new(
        ctx: egui::Context,
        backend: Box<dyn Backend>,
        events: Receiver<BackendEvent>,
        config: Config,
        startup_warnings: Vec<String>,
    ) -> Self {
        let history = backend.load_history();
        let bindings = Bindings::from_config(&config.keybindings);
        let banners = startup_warnings
            .into_iter()
            .chain(bindings.warnings.iter().cloned())
            .map(|text| Banner { text })
            .collect();
        let mut app = Self {
            ctx,
            backend,
            events,
            config,
            bindings,
            banners,
            tabs: vec![TabSlot::default()],
            active_tab: 0,
            search: SearchState::default(),
            results: None,
            selection: Selection::default(),
            scroll_to_row: None,
            visible_rows: 20,
            cache: RowCache::default(),
            actions: Arc::default(),
            actions_loaded_at: 0.0,
            context_extras: ContextExtras::default(),
            snapshot: Arc::new(IndexSnapshot::empty()),
            index: IndexStatus::default(),
            volumes: Vec::new(),
            toasts: Vec::new(),
            windows: Windows::default(),
            preview: PreviewState::default(),
            rename: None,
            pending_reselect: None,
            selected_since: None,
            slow_click: None,
            now: 0.0,
            pointer: None,
            quit_requested: false,
            duplicates_only: false,
            history,
            dropdown: history::HistoryDropdown::default(),
        };
        app.apply_theme();
        app.backend.load_actions();
        app.resubmit_search();
        app
    }

    /// Whether the index holds no entries at all.
    pub(crate) fn index_is_empty(&self) -> bool {
        self.snapshot.is_empty()
    }
}

#[cfg(test)]
pub(crate) mod tests;

//! The application state machine.
//!
//! [`App`] owns all UI state. Everything that blocks goes through the [`Backend`]; results come back
//! as [`BackendEvent`]s which [`App::poll_events`] folds into the state at the start of each frame.
//! The egui rendering lives in `crate::ui`; the logic in the submodules here is free of widgets and
//! is exercised directly by the tests.

mod commands;
mod events;
mod frame;
mod interact;
mod search;
mod settings;
mod windows;

use std::sync::Arc;

use crossbeam_channel::Receiver;
use egui::Pos2;
use everythux_core::config::Config;
use everythux_core::model::VolumeInfo;
use everythux_core::snapshot::IndexSnapshot;

pub(crate) use commands::Command;
pub(crate) use frame::search_id;
pub(crate) use interact::{CellEvent, ContextAction, CopyKind, Effect};
pub(crate) use settings::valid_rules;

use crate::backend::{Backend, BackendEvent};
use crate::exclusions::{ExclusionEditor, ExclusionPopup};
use crate::keys::Bindings;
use crate::model::ResultsView;
use crate::preview::PreviewState;
use crate::properties::PropertiesWindow;
use crate::selection::Selection;
use crate::status::{IndexStatus, Toast};
use crate::ui::table::RowCache;
use crate::volumes::VolumesWindow;

/// Row height of the result table in logical pixels.
pub(crate) const ROW_HEIGHT: f32 = 20.0;

/// Search box state.
#[derive(Debug, Default)]
pub(crate) struct SearchState {
    /// Current text of the search box.
    pub(crate) text: String,
    /// Generation of the most recently submitted request; older results are ignored.
    pub(crate) latest_generation: u64,
    /// Parse error of the last query, shown under the box.
    pub(crate) error: Option<String>,
    /// Focus the box on the next frame.
    pub(crate) focus_requested: bool,
}

/// Identifies a window or popup for the Escape-key stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PopupId {
    ConfirmReindex,
    ExclusionPopup,
    ExclusionEditor,
    Volumes,
    Properties(u64),
    Shortcuts,
    About,
}

/// All secondary windows. `order` is the z-order, topmost last.
#[derive(Debug, Default)]
pub(crate) struct Windows {
    pub(crate) confirm_reindex: bool,
    pub(crate) exclusion_popup: Option<ExclusionPopup>,
    pub(crate) exclusion_editor: ExclusionEditor,
    pub(crate) volumes: VolumesWindow,
    pub(crate) properties: Vec<PropertiesWindow>,
    pub(crate) shortcuts: bool,
    pub(crate) about: bool,
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

    pub(crate) search: SearchState,
    pub(crate) results: Option<ResultsView>,
    pub(crate) selection: Selection,
    /// Row the table scrolls to on the next frame (keyboard navigation).
    pub(crate) scroll_to_row: Option<usize>,
    /// Rows that fit in the table viewport (for Page Up/Down); updated by the table.
    pub(crate) visible_rows: usize,
    pub(crate) cache: RowCache,

    /// Latest snapshot published by the indexer.
    pub(crate) snapshot: Arc<IndexSnapshot>,
    pub(crate) index: IndexStatus,
    pub(crate) volumes: Vec<VolumeInfo>,

    pub(crate) toasts: Vec<Toast>,
    pub(crate) windows: Windows,
    pub(crate) preview: PreviewState,

    /// `egui` time of the current frame, seconds.
    pub(crate) now: f64,
    /// Latest pointer position, used to place popups near the cursor.
    pub(crate) pointer: Option<Pos2>,
    /// Set when the app asked the viewport to close.
    pub(crate) quit_requested: bool,
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
            search: SearchState {
                focus_requested: true,
                ..SearchState::default()
            },
            results: None,
            selection: Selection::default(),
            scroll_to_row: None,
            visible_rows: 20,
            cache: RowCache::default(),
            snapshot: Arc::new(IndexSnapshot::empty()),
            index: IndexStatus::default(),
            volumes: Vec::new(),
            toasts: Vec::new(),
            windows: Windows::default(),
            preview: PreviewState::default(),
            now: 0.0,
            pointer: None,
            quit_requested: false,
        };
        app.apply_theme();
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

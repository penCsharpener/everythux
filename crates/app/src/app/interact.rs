//! Row interaction: what clicks, double-clicks, keys and context-menu entries do.
//!
//! Handlers are pure state transitions that return [`Effect`]s; [`App::apply_effects`] turns those
//! into backend calls. This keeps the per-cell routing (the heart of requirements R6, R7 and R9)
//! testable without a UI.

use std::path::PathBuf;

use egui::Modifiers;
use everythux_core::config::ColumnId;
use everythux_index::{Job, ScanMode};

use super::App;
use super::rename::SLOW_CLICK_AFTER;
use crate::backend::PlatformAction;
use crate::model::PropTarget;
use crate::status::ToastKind;

/// Most files `Enter` / `Ctrl+Enter` act on at once, so a stray `Ctrl+A`-style selection can't
/// launch hundreds of applications.
const MAX_OPEN_AT_ONCE: usize = 25;

/// Something the user did to a table cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CellEvent {
    /// Primary-button click with the modifiers that were held.
    Click(Modifiers),
    /// Primary-button double-click.
    DoubleClick(Modifiers),
    /// Secondary-button click (opens the context menu).
    Secondary,
}

/// A side effect decided by a handler.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Effect {
    OpenDefault(PathBuf),
    ShowItem(PathBuf),
    ShowProperties(PropTarget),
    /// Add an exclusion for this folder and open the popup.
    ExcludeFolder(PathBuf),
    Reindex {
        path: PathBuf,
        mode: ScanMode,
    },
    CopyText(String),
    /// Run an "Open with" application or a file action on these paths.
    Launch {
        launcher: everythux_platform::actions::Launcher,
        paths: Vec<PathBuf>,
    },
    /// Put these files on the clipboard as a file list.
    CopyFiles(Vec<PathBuf>),
    /// Put these files on the clipboard as a cut list.
    CutFiles(Vec<PathBuf>),
    /// Paste the clipboard's files into this folder.
    PasteInto(PathBuf),
    /// Move these to the trash.
    Trash(Vec<PathBuf>),
    /// Ask before deleting these permanently.
    ConfirmDelete(Vec<PathBuf>),
}

/// Keyboard navigation in the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Nav {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

/// What to copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyKind {
    FullPath,
    Name,
    FolderPath,
}

/// Entries of the row context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextAction {
    Open,
    OpenFolder,
    Copy(CopyKind),
    Properties,
    ExcludeFolder,
    Reindex(ScanMode),
    Rename,
    /// Run entry `n` of `App::context_extras`.
    Launch(usize),
    /// Hide entry `n` of `App::context_extras` from the menu.
    HideEntry(usize),
    /// Copy the selected files to the clipboard as a file list.
    CopyFiles,
    /// Cut the selected files to the clipboard (paste moves them).
    CutFiles,
    /// Paste the clipboard's files into the right-clicked folder.
    Paste,
    /// Move the selection to the trash.
    Trash,
    /// Delete the selection permanently, after a confirmation.
    Delete,
}

impl App {
    /// Handles a click or double-click on the cell at (`row`, `column`).
    pub(crate) fn on_cell_event(&mut self, row: usize, column: ColumnId, event: CellEvent) -> Vec<Effect> {
        let Some(results) = &self.results else {
            return Vec::new();
        };
        if row >= results.len() {
            return Vec::new();
        }
        match event {
            CellEvent::Click(mods) => self.on_cell_click(row, column, mods),
            CellEvent::DoubleClick(mods) => {
                self.slow_click = None;
                // A Ctrl+Shift double-click is two exclusion clicks, not an "open".
                if column == ColumnId::Path
                    && self
                        .bindings
                        .exclude_click
                        .matches(egui::PointerButton::Primary, mods)
                {
                    return self.on_cell_click(row, column, mods);
                }
                self.selection.select_only(row);
                self.double_click_effects(row, column)
            }
            CellEvent::Secondary => {
                if !self.selection.contains(row) {
                    self.selection.select_only(row);
                }
                self.refresh_context_extras();
                Vec::new()
            }
        }
    }

    fn on_cell_click(&mut self, row: usize, column: ColumnId, mods: Modifiers) -> Vec<Effect> {
        if column == ColumnId::Path
            && self
                .bindings
                .exclude_click
                .matches(egui::PointerButton::Primary, mods)
        {
            self.selection.select_only(row);
            return self
                .results
                .as_ref()
                .and_then(|r| r.folder_path(row))
                .map(Effect::ExcludeFolder)
                .into_iter()
                .collect();
        }
        self.slow_click = None;
        if mods.shift && !mods.ctrl && !mods.command {
            self.selection.select_range_to(row);
            self.selected_since = None;
        } else if mods.ctrl || mods.command {
            self.selection.toggle(row);
            self.selected_since = None;
        } else {
            let already_selected = self.selection.len() == 1 && self.selection.contains(row);
            let selected_long_ago = self
                .selected_since
                .is_some_and(|(r, at)| r == row && self.now - at > SLOW_CLICK_AFTER);
            if column == ColumnId::Name
                && already_selected
                && selected_long_ago
                && self.config.ui.rename_on_slow_click
                && self.rename.is_none()
            {
                // Wait out the double-click interval first: a double-click opens the file.
                self.slow_click = Some((row, self.now));
                self.tick_slow_click();
            } else if !already_selected {
                self.selection.select_only(row);
                self.selected_since = Some((row, self.now));
            }
        }
        Vec::new()
    }

    fn double_click_effects(&self, row: usize, column: ColumnId) -> Vec<Effect> {
        let Some(results) = &self.results else {
            return Vec::new();
        };
        match column {
            ColumnId::Name | ColumnId::Type => results.full_path(row).map(Effect::OpenDefault),
            ColumnId::Path => results.full_path(row).map(Effect::ShowItem),
            ColumnId::Size | ColumnId::Modified => results
                .prop_target(row, self.config.ui.properties_target)
                .map(Effect::ShowProperties),
        }
        .into_iter()
        .collect()
    }

    /// Effects of a context-menu entry chosen on `row`.
    pub(crate) fn context_effects(&mut self, row: usize, action: ContextAction) -> Vec<Effect> {
        let Some(results) = &self.results else {
            return Vec::new();
        };
        let single = |effect: Option<Effect>| effect.into_iter().collect::<Vec<_>>();
        match action {
            ContextAction::Open => single(results.full_path(row).map(Effect::OpenDefault)),
            ContextAction::OpenFolder => single(results.full_path(row).map(Effect::ShowItem)),
            ContextAction::Copy(kind) => {
                let rows = if self.selection.contains(row) {
                    self.selection.rows().collect()
                } else {
                    vec![row]
                };
                self.copy_effects(&rows, kind)
            }
            ContextAction::Properties => single(
                results
                    .prop_target(row, self.config.ui.properties_target)
                    .map(Effect::ShowProperties),
            ),
            ContextAction::ExcludeFolder => single(self.exclusion_target(row).map(Effect::ExcludeFolder)),
            ContextAction::Rename => {
                self.selection.select_only(row);
                self.start_rename(row);
                Vec::new()
            }
            ContextAction::Launch(index) => {
                let Some(entry) = self.context_extras.entries.get(index) else {
                    return Vec::new();
                };
                single(Some(Effect::Launch {
                    launcher: entry.launcher.clone(),
                    paths: self.context_extras.paths.clone(),
                }))
            }
            ContextAction::HideEntry(index) => {
                self.hide_context_entry(index);
                Vec::new()
            }
            ContextAction::CopyFiles => {
                let paths = self.context_paths(row);
                if paths.is_empty() {
                    Vec::new()
                } else {
                    vec![Effect::CopyFiles(paths)]
                }
            }
            ContextAction::CutFiles => {
                let paths = self.context_paths(row);
                if paths.is_empty() {
                    Vec::new()
                } else {
                    vec![Effect::CutFiles(paths)]
                }
            }
            ContextAction::Paste => single(
                results
                    .is_dir(row)
                    .then(|| results.full_path(row))
                    .flatten()
                    .map(Effect::PasteInto),
            ),
            ContextAction::Trash => {
                let paths = self.context_paths(row);
                if paths.is_empty() {
                    Vec::new()
                } else {
                    vec![Effect::Trash(paths)]
                }
            }
            ContextAction::Delete => {
                let paths = self.context_paths(row);
                if paths.is_empty() {
                    Vec::new()
                } else {
                    vec![Effect::ConfirmDelete(paths)]
                }
            }
            ContextAction::Reindex(mode) => single(
                self.reindex_target(row)
                    .map(|path| Effect::Reindex { path, mode }),
            ),
        }
    }

    /// The paths a context-menu entry acts on: the selection if `row` is part of it, else `row`.
    fn context_paths(&self, row: usize) -> Vec<PathBuf> {
        let Some(results) = &self.results else {
            return Vec::new();
        };
        if self.selection.contains(row) {
            self.selection
                .rows()
                .filter_map(|r| results.full_path(r))
                .collect()
        } else {
            results.full_path(row).into_iter().collect()
        }
    }

    /// The folder "Exclude folder…" acts on: a folder row excludes itself, other rows their
    /// containing folder (the same folder the Path cell shows).
    pub(crate) fn exclusion_target(&self, row: usize) -> Option<PathBuf> {
        let results = self.results.as_ref()?;
        if results.is_dir(row) {
            results.full_path(row)
        } else {
            results.folder_path(row)
        }
    }

    /// The folder "Re-index this folder" acts on: the folder itself for folder rows, else the parent.
    pub(crate) fn reindex_target(&self, row: usize) -> Option<PathBuf> {
        let results = self.results.as_ref()?;
        if results.is_dir(row) {
            results.full_path(row)
        } else {
            results.folder_path(row)
        }
    }

    fn copy_effects(&self, rows: &[usize], kind: CopyKind) -> Vec<Effect> {
        let Some(results) = &self.results else {
            return Vec::new();
        };
        let mut lines: Vec<String> = Vec::with_capacity(rows.len());
        for &row in rows {
            let Some(id) = results.entry(row) else { continue };
            let text = match kind {
                CopyKind::FullPath => results.snapshot.full_path(id).to_string_lossy().into_owned(),
                CopyKind::Name => String::from_utf8_lossy(results.snapshot.name(id)).into_owned(),
                CopyKind::FolderPath => results.snapshot.parent_path(id).to_string_lossy().into_owned(),
            };
            if kind == CopyKind::FolderPath && lines.last() == Some(&text) {
                continue;
            }
            lines.push(text);
        }
        if lines.is_empty() {
            Vec::new()
        } else {
            vec![Effect::CopyText(lines.join("\n"))]
        }
    }

    /// The full paths of the selected rows.
    pub(crate) fn selected_paths(&self) -> Vec<PathBuf> {
        let Some(results) = &self.results else {
            return Vec::new();
        };
        self.selection
            .rows()
            .filter_map(|row| results.full_path(row))
            .collect()
    }

    /// `Delete`: move the selection to the trash.
    pub(crate) fn trash_selection(&self) -> Vec<Effect> {
        let paths = self.selected_paths();
        if paths.is_empty() {
            Vec::new()
        } else {
            vec![Effect::Trash(paths)]
        }
    }

    /// Opens the "Delete permanently?" dialog for `paths` (nothing happens for an empty list).
    pub(crate) fn request_delete(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        self.windows.confirm_delete = Some(paths);
        self.open_popup(super::PopupId::ConfirmDelete);
    }

    /// "Delete" in the confirmation dialog: hands the waiting paths to the backend.
    pub(crate) fn confirm_delete(&mut self) {
        if let Some(paths) = self.windows.confirm_delete.take() {
            self.windows.order.retain(|p| *p != super::PopupId::ConfirmDelete);
            self.backend.platform(PlatformAction::Delete(paths));
        }
    }

    /// `Ctrl+C` / `Ctrl+Shift+C` on the selection.
    pub(crate) fn copy_selection(&self, kind: CopyKind) -> Vec<Effect> {
        let rows: Vec<usize> = self.selection.rows().collect();
        self.copy_effects(&rows, kind)
    }

    /// `Enter`: open the selected entries with their default applications.
    pub(crate) fn open_selection(&self) -> Vec<Effect> {
        let Some(results) = &self.results else {
            return Vec::new();
        };
        self.selection
            .rows()
            .take(MAX_OPEN_AT_ONCE)
            .filter_map(|row| results.full_path(row))
            .map(Effect::OpenDefault)
            .collect()
    }

    /// `Ctrl+Enter`: show the selected entries in the file manager.
    pub(crate) fn show_selection_in_folder(&self) -> Vec<Effect> {
        let Some(results) = &self.results else {
            return Vec::new();
        };
        self.selection
            .rows()
            .take(MAX_OPEN_AT_ONCE)
            .filter_map(|row| results.full_path(row))
            .map(Effect::ShowItem)
            .collect()
    }

    /// Moves the selection cursor; `extend` (Shift held) grows the selection.
    pub(crate) fn navigate(&mut self, nav: Nav, extend: bool) {
        let Some(len) = self.results.as_ref().map(crate::model::ResultsView::len) else {
            return;
        };
        if len == 0 {
            return;
        }
        let last = len - 1;
        let page = self.visible_rows.saturating_sub(1).max(1);
        let target = match (self.selection.cursor(), nav) {
            (_, Nav::Home) | (None, Nav::Down | Nav::PageDown) => 0,
            (_, Nav::End) | (None, Nav::Up | Nav::PageUp) => last,
            (Some(c), Nav::Up) => c.saturating_sub(1),
            (Some(c), Nav::Down) => (c + 1).min(last),
            (Some(c), Nav::PageUp) => c.saturating_sub(page),
            (Some(c), Nav::PageDown) => (c + page).min(last),
        };
        self.selection.move_to(target, extend);
        self.selected_since = Some((target, self.now));
        self.scroll_to_row = Some(target);
    }

    /// Executes effects by talking to the backend.
    pub(crate) fn apply_effects(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::OpenDefault(path) => {
                    self.record_history();
                    self.backend.platform(PlatformAction::OpenDefault(path));
                }
                Effect::ShowItem(path) => self.backend.platform(PlatformAction::ShowItem(path)),
                Effect::ShowProperties(target) => {
                    self.backend.platform(PlatformAction::ShowProperties(target));
                }
                Effect::ExcludeFolder(folder) => self.begin_exclusion(folder),
                Effect::Reindex { path, mode } => {
                    let label = match mode {
                        ScanMode::Quick => "quick",
                        ScanMode::Full => "full",
                    };
                    self.toast(
                        format!("Re-indexing {} ({label})", path.display()),
                        ToastKind::Info,
                    );
                    self.backend.submit_job(Job::RescanPath { path, mode });
                }
                Effect::Launch { launcher, paths } => {
                    self.backend.platform(PlatformAction::Launch { launcher, paths });
                }
                Effect::CopyFiles(paths) => self.backend.platform(PlatformAction::CopyFiles(paths)),
                Effect::CutFiles(paths) => self.backend.platform(PlatformAction::CutFiles(paths)),
                Effect::PasteInto(dest) => self.backend.platform(PlatformAction::Paste { dest }),
                Effect::Trash(paths) => self.backend.platform(PlatformAction::Trash(paths)),
                Effect::ConfirmDelete(paths) => self.request_delete(paths),
                Effect::CopyText(text) => {
                    let count = text.lines().count();
                    self.ctx.copy_text(text);
                    self.toast(
                        if count == 1 {
                            "Copied to clipboard".to_owned()
                        } else {
                            format!("Copied {count} lines to clipboard")
                        },
                        ToastKind::Info,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::Modifiers;
    use everythux_core::config::PropertiesTarget;
    use everythux_core::exclusion::{ExclusionRule, RuleKind};

    use super::*;
    use crate::app::tests::TestApp;
    use crate::backend::fake::Call;

    const NONE: Modifiers = Modifiers::NONE;

    /// Rows: 0 = docs/notes.txt? The fixture decides; use `t.row_of`.
    fn setup() -> (TestApp, usize) {
        let mut t = TestApp::new();
        t.show_results(&[
            t.fixture.report,
            t.fixture.notes,
            t.fixture.junk,
            t.fixture.docs_entry,
        ]);
        (t, 0)
    }

    #[test]
    fn double_click_name_opens_the_file() {
        let (mut t, row) = setup();
        let fx = t
            .app
            .on_cell_event(row, ColumnId::Name, CellEvent::DoubleClick(NONE));
        assert_eq!(fx, [Effect::OpenDefault("/home/me/docs/report.pdf".into())]);
        t.app.apply_effects(fx);
        assert_eq!(
            t.fake.platform_actions(),
            [PlatformAction::OpenDefault("/home/me/docs/report.pdf".into())]
        );
    }

    #[test]
    fn double_click_path_shows_the_item_in_its_folder() {
        let (mut t, row) = setup();
        let fx = t
            .app
            .on_cell_event(row, ColumnId::Path, CellEvent::DoubleClick(NONE));
        assert_eq!(fx, [Effect::ShowItem("/home/me/docs/report.pdf".into())]);
        t.app.apply_effects(fx);
        assert_eq!(
            t.fake.platform_actions(),
            [PlatformAction::ShowItem("/home/me/docs/report.pdf".into())]
        );
    }

    #[test]
    fn double_click_size_or_modified_shows_the_containing_folder_properties() {
        let (mut t, row) = setup();
        for column in [ColumnId::Size, ColumnId::Modified] {
            let fx = t.app.on_cell_event(row, column, CellEvent::DoubleClick(NONE));
            let [Effect::ShowProperties(target)] = fx.as_slice() else {
                panic!("expected ShowProperties for {column:?}, got {fx:?}");
            };
            assert_eq!(target.path, PathBuf::from("/home/me/docs"), "{column:?}");
            assert!(target.kind.is_dir());
            assert_eq!(target.size, 2048 + 10, "recursive total of the folder");
        }
    }

    #[test]
    fn properties_target_item_uses_the_row_itself() {
        let (mut t, row) = setup();
        t.app.config.ui.properties_target = PropertiesTarget::Item;
        let fx = t
            .app
            .on_cell_event(row, ColumnId::Size, CellEvent::DoubleClick(NONE));
        let [Effect::ShowProperties(target)] = fx.as_slice() else {
            panic!("{fx:?}");
        };
        assert_eq!(target.path, PathBuf::from("/home/me/docs/report.pdf"));
        assert_eq!(target.size, 2048);
    }

    #[test]
    fn folder_rows_show_their_own_properties() {
        let (mut t, _) = setup();
        let fx = t
            .app
            .on_cell_event(3, ColumnId::Modified, CellEvent::DoubleClick(NONE));
        let [Effect::ShowProperties(target)] = fx.as_slice() else {
            panic!("{fx:?}");
        };
        assert_eq!(target.path, PathBuf::from("/home/me/docs"));
    }

    #[test]
    fn clicks_select_with_ctrl_and_shift() {
        let (mut t, _) = setup();
        t.app.on_cell_event(0, ColumnId::Name, CellEvent::Click(NONE));
        t.app
            .on_cell_event(2, ColumnId::Name, CellEvent::Click(Modifiers::CTRL));
        assert_eq!(t.app.selection.rows().collect::<Vec<_>>(), [0, 2]);
        t.app
            .on_cell_event(3, ColumnId::Size, CellEvent::Click(Modifiers::SHIFT));
        assert_eq!(t.app.selection.rows().collect::<Vec<_>>(), [2, 3]);
        t.app.on_cell_event(1, ColumnId::Name, CellEvent::Click(NONE));
        assert_eq!(t.app.selection.rows().collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn ctrl_shift_click_on_path_requests_an_exclusion() {
        let (mut t, row) = setup();
        let mods = Modifiers::CTRL | Modifiers::SHIFT;
        let fx = t.app.on_cell_event(row, ColumnId::Path, CellEvent::Click(mods));
        assert_eq!(fx, [Effect::ExcludeFolder("/home/me/docs".into())]);
    }

    #[test]
    fn ctrl_shift_click_elsewhere_is_not_an_exclusion() {
        let (mut t, row) = setup();
        let mods = Modifiers::CTRL | Modifiers::SHIFT;
        for column in [ColumnId::Name, ColumnId::Type, ColumnId::Modified, ColumnId::Size] {
            let fx = t.app.on_cell_event(row, column, CellEvent::Click(mods));
            assert!(fx.is_empty(), "{column:?}: {fx:?}");
        }
    }

    #[test]
    fn ctrl_shift_double_click_on_path_does_not_open_the_folder() {
        let (mut t, row) = setup();
        let mods = Modifiers::CTRL | Modifiers::SHIFT;
        let fx = t
            .app
            .on_cell_event(row, ColumnId::Path, CellEvent::DoubleClick(mods));
        assert_eq!(fx, [Effect::ExcludeFolder("/home/me/docs".into())]);
    }

    #[test]
    fn excluding_applies_a_path_rule_and_opens_the_popup() {
        let (mut t, row) = setup();
        let mods = Modifiers::CTRL | Modifiers::SHIFT;
        let fx = t.app.on_cell_event(row, ColumnId::Path, CellEvent::Click(mods));
        t.app.apply_effects(fx);
        let rule = ExclusionRule::new("/home/me/docs", RuleKind::Path);
        assert!(t.app.config.exclusions.contains(&rule), "rule added immediately");
        let popup = t.app.windows.exclusion_popup.as_ref().expect("popup open");
        assert_eq!(popup.folder, PathBuf::from("/home/me/docs"));
        let saved = t.fake.last_saved_config().expect("config saved");
        assert!(saved.exclusions.contains(&rule));
        let settings = t.fake.last_settings().expect("indexer informed");
        assert!(settings.exclusions.contains(&rule));
    }

    #[test]
    fn keyboard_navigation_moves_and_clamps() {
        let (mut t, _) = setup();
        t.app.visible_rows = 3;
        t.app.navigate(Nav::Down, false);
        assert_eq!(
            t.app.selection.cursor(),
            Some(0),
            "first Down selects the first row"
        );
        t.app.navigate(Nav::Down, false);
        t.app.navigate(Nav::Down, false);
        assert_eq!(t.app.selection.cursor(), Some(2));
        t.app.navigate(Nav::PageDown, false);
        assert_eq!(t.app.selection.cursor(), Some(3), "clamped to the last row");
        t.app.navigate(Nav::PageUp, false);
        assert_eq!(t.app.selection.cursor(), Some(1));
        t.app.navigate(Nav::Home, false);
        assert_eq!(t.app.selection.cursor(), Some(0));
        t.app.navigate(Nav::End, true);
        assert_eq!(t.app.selection.len(), 4, "shift extends from the anchor");
        assert_eq!(t.app.scroll_to_row, Some(3));
        t.app.navigate(Nav::Up, false);
        assert_eq!(t.app.selection.rows().collect::<Vec<_>>(), [2]);
    }

    #[test]
    fn enter_and_ctrl_enter_act_on_the_selection() {
        let (mut t, _) = setup();
        t.app.on_cell_event(1, ColumnId::Name, CellEvent::Click(NONE));
        assert_eq!(
            t.app.open_selection(),
            [Effect::OpenDefault("/home/me/docs/notes.txt".into())]
        );
        assert_eq!(
            t.app.show_selection_in_folder(),
            [Effect::ShowItem("/home/me/docs/notes.txt".into())]
        );
    }

    #[test]
    fn copy_paths_and_folders() {
        let (mut t, _) = setup();
        t.app.on_cell_event(0, ColumnId::Name, CellEvent::Click(NONE));
        t.app
            .on_cell_event(1, ColumnId::Name, CellEvent::Click(Modifiers::SHIFT));
        assert_eq!(
            t.app.copy_selection(CopyKind::FullPath),
            [Effect::CopyText(
                "/home/me/docs/report.pdf\n/home/me/docs/notes.txt".into()
            )]
        );
        assert_eq!(
            t.app.copy_selection(CopyKind::FolderPath),
            [Effect::CopyText("/home/me/docs".into())],
            "same folder is listed once"
        );
    }

    #[test]
    fn context_menu_actions() {
        let (mut t, _) = setup();
        assert_eq!(
            t.app.context_effects(2, ContextAction::Copy(CopyKind::Name)),
            [Effect::CopyText("junk.js".into())]
        );
        assert_eq!(
            t.app.context_effects(2, ContextAction::OpenFolder),
            [Effect::ShowItem("/home/me/proj/node_modules/junk.js".into())]
        );
        assert_eq!(
            t.app.context_effects(2, ContextAction::Reindex(ScanMode::Full)),
            [Effect::Reindex {
                path: "/home/me/proj/node_modules".into(),
                mode: ScanMode::Full
            }],
            "file rows re-index their parent"
        );
        assert_eq!(
            t.app.context_effects(3, ContextAction::Reindex(ScanMode::Quick)),
            [Effect::Reindex {
                path: "/home/me/docs".into(),
                mode: ScanMode::Quick
            }],
            "folder rows re-index themselves"
        );
        assert_eq!(
            t.app.context_effects(3, ContextAction::ExcludeFolder),
            [Effect::ExcludeFolder("/home/me/docs".into())]
        );
        t.app.apply_effects(vec![Effect::Reindex {
            path: "/home/me/docs".into(),
            mode: ScanMode::Quick,
        }]);
        assert_eq!(
            t.fake.calls().last(),
            Some(&Call::SubmitJob(Job::RescanPath {
                path: "/home/me/docs".into(),
                mode: ScanMode::Quick
            }))
        );
    }

    #[test]
    fn right_click_selects_an_unselected_row_only() {
        let (mut t, _) = setup();
        t.app.on_cell_event(0, ColumnId::Name, CellEvent::Click(NONE));
        t.app
            .on_cell_event(1, ColumnId::Name, CellEvent::Click(Modifiers::CTRL));
        t.app.on_cell_event(1, ColumnId::Name, CellEvent::Secondary);
        assert_eq!(t.app.selection.len(), 2, "kept the multi-selection");
        t.app.on_cell_event(3, ColumnId::Name, CellEvent::Secondary);
        assert_eq!(t.app.selection.rows().collect::<Vec<_>>(), [3]);
    }
}

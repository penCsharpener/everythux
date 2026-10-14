//! The virtualised result table.
//!
//! Only visible rows are touched: `TableBody::rows` asks for the visible window, formatted strings
//! are cached per row and dropped when the results or the format settings change.

use std::collections::HashMap;

use egui::{Align, Layout, Modifiers, RichText, Sense, Shape, Stroke, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use everythux_core::config::{ColumnConfig, ColumnId, SizeFormat, SortConfig};
use everythux_core::model::{EntryKind, VolumeState};
use everythux_index::ScanMode;
use everythux_platform::mime::Category;

use super::icons;
use super::rename_editor::{EditorEvent, rename_editor};
use crate::app::CopyKind;
use crate::app::{App, CellEvent, ContextAction, ROW_HEIGHT};
use crate::backend::Backend;
use crate::format::{format_local, format_size};
use crate::model::ResultsView;
use crate::rename::RenameEdit;
use crate::selection::Selection;

const HEADER_HEIGHT: f32 = 22.0;
/// Upper bound on cached rows; a screenful is ~50, so this only matters after long scrolls.
const MAX_CACHED_ROWS: usize = 2048;

/// Display strings of one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CachedRow {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) kind: String,
    pub(crate) modified: String,
    pub(crate) size: String,
    pub(crate) category: Category,
    /// Entry belongs to a volume that is currently offline.
    pub(crate) offline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheKey {
    epoch: u64,
    size_format: SizeFormat,
    date_format: String,
}

/// Formatted strings for the rows that were visible recently.
#[derive(Debug, Default)]
pub(crate) struct RowCache {
    epoch: u64,
    key: Option<CacheKey>,
    rows: HashMap<usize, CachedRow>,
}

impl RowCache {
    /// Forgets all rows (called when the result list changes).
    pub(crate) fn invalidate(&mut self) {
        self.epoch += 1;
        self.rows.clear();
    }

    /// Number of cached rows (for tests and diagnostics).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    fn get(
        &mut self,
        row: usize,
        size_format: SizeFormat,
        date_format: &str,
        build: impl FnOnce() -> CachedRow,
    ) -> &CachedRow {
        let key_matches = self.key.as_ref().is_some_and(|k| {
            k.epoch == self.epoch && k.size_format == size_format && k.date_format == date_format
        });
        if !key_matches {
            self.rows.clear();
            self.key = Some(CacheKey {
                epoch: self.epoch,
                size_format,
                date_format: date_format.to_owned(),
            });
        }
        if self.rows.len() >= MAX_CACHED_ROWS && !self.rows.contains_key(&row) {
            self.rows.clear();
        }
        self.rows.entry(row).or_insert_with(build)
    }
}

/// Builds the display strings of `row`.
pub(crate) fn build_row(
    view: &ResultsView,
    row: usize,
    backend: &dyn Backend,
    size_format: SizeFormat,
    date_format: &str,
) -> CachedRow {
    let snap = &*view.snapshot;
    let id = view.ids[row];
    let i = id.index();
    let kind = snap.entries.kind[i];
    let (type_name, category) = match kind {
        EntryKind::Dir => ("Folder".to_owned(), Category::Folder),
        EntryKind::Symlink => ("Symbolic link".to_owned(), Category::Other),
        EntryKind::Other => ("Special file".to_owned(), Category::Other),
        EntryKind::File => {
            let ext = snap.extension(id);
            (backend.describe_extension(ext).to_string(), backend.category(ext))
        }
    };
    let volume = snap.dirs.volume[snap.entries.dir[i].index()];
    let offline = snap
        .volumes
        .get(volume.index())
        .is_some_and(|v| v.state == VolumeState::Offline);
    CachedRow {
        name: String::from_utf8_lossy(snap.name(id)).into_owned(),
        path: snap.parent_path(id).to_string_lossy().into_owned(),
        kind: type_name,
        modified: format_local(snap.entries.mtime_ns[i], date_format),
        size: format_size(snap.entries.size[i], size_format),
        category,
        offline,
    }
}

/// Everything the table reports back to the app.
#[derive(Debug, Default)]
pub(crate) struct TableOutput {
    pub(crate) cell_events: Vec<(usize, ColumnId, CellEvent)>,
    pub(crate) header_click: Option<ColumnId>,
    pub(crate) widths: Vec<(ColumnId, f32)>,
    pub(crate) context: Vec<(usize, ContextAction)>,
    /// What the rename editor reported this frame.
    pub(crate) editor: Option<EditorEvent>,
    /// The rename editor was drawn this frame.
    pub(crate) editor_drawn: bool,
}

struct TableParams<'a> {
    view: &'a ResultsView,
    columns: &'a [ColumnConfig],
    selection: &'a Selection,
    sort: SortConfig,
    size_format: SizeFormat,
    date_format: &'a str,
    scroll_to: Option<usize>,
    backend: &'a dyn Backend,
}

fn header_cell(ui: &mut Ui, id: ColumnId, sort: SortConfig) {
    ui.add(
        egui::Label::new(RichText::new(id.label()).strong())
            .selectable(false)
            .truncate(),
    );
    if sort.column == id {
        let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
        let c = rect.center();
        let color = ui.visuals().text_color();
        let points = if sort.ascending {
            vec![c + vec2(0.0, -3.5), c + vec2(4.0, 3.0), c + vec2(-4.0, 3.0)]
        } else {
            vec![c + vec2(0.0, 3.5), c + vec2(4.0, -3.0), c + vec2(-4.0, -3.0)]
        };
        ui.painter()
            .add(Shape::convex_polygon(points, color, Stroke::NONE));
    }
}

fn text_cell(ui: &mut Ui, text: &str, offline: bool) {
    let mut rich = RichText::new(text);
    if offline {
        rich = rich.color(ui.visuals().weak_text_color());
    }
    ui.add(egui::Label::new(rich).selectable(false).truncate());
}

fn cell_content(ui: &mut Ui, id: ColumnId, row: &CachedRow) {
    match id {
        ColumnId::Name => {
            icons::icon(ui, row.category, row.offline);
            text_cell(ui, &row.name, row.offline);
        }
        ColumnId::Path => text_cell(ui, &row.path, row.offline),
        ColumnId::Type => text_cell(ui, &row.kind, row.offline),
        ColumnId::Modified => text_cell(ui, &row.modified, row.offline),
        ColumnId::Size => {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                text_cell(ui, &row.size, row.offline);
            });
        }
    }
}

fn pick(ui: &mut Ui, out: &mut Vec<(usize, ContextAction)>, row: usize, label: &str, action: ContextAction) {
    if ui.button(label).clicked() {
        out.push((row, action));
        ui.close();
    }
}

fn context_menu(ui: &mut Ui, row: usize, renamable: bool, out: &mut Vec<(usize, ContextAction)>) {
    pick(ui, out, row, "Open", ContextAction::Open);
    pick(ui, out, row, "Open containing folder", ContextAction::OpenFolder);
    ui.separator();
    pick(
        ui,
        out,
        row,
        "Copy full path",
        ContextAction::Copy(CopyKind::FullPath),
    );
    pick(ui, out, row, "Copy name", ContextAction::Copy(CopyKind::Name));
    pick(
        ui,
        out,
        row,
        "Copy folder path",
        ContextAction::Copy(CopyKind::FolderPath),
    );
    ui.separator();
    let rename = ui
        .add_enabled(renamable, egui::Button::new("Rename…").shortcut_text("F2"))
        .on_disabled_hover_text("Rename needs a single item on an online volume");
    if rename.clicked() {
        out.push((row, ContextAction::Rename));
        ui.close();
    }
    pick(ui, out, row, "Properties…", ContextAction::Properties);
    pick(ui, out, row, "Exclude folder…", ContextAction::ExcludeFolder);
    ui.menu_button("Re-index this folder", |ui| {
        pick(
            ui,
            out,
            row,
            "Quick (changed folders only)",
            ContextAction::Reindex(ScanMode::Quick),
        );
        pick(ui, out, row, "Full", ContextAction::Reindex(ScanMode::Full));
    });
}

fn draw_table(
    ui: &mut Ui,
    p: &TableParams<'_>,
    cache: &mut RowCache,
    rename: &mut Option<RenameEdit>,
) -> TableOutput {
    let mut out = TableOutput::default();
    ui.style_mut().interaction.selectable_labels = false;

    let salt = egui::Id::new((
        "results_table",
        p.columns.iter().map(|c| c.id).collect::<Vec<_>>(),
    ));
    let mut builder = TableBuilder::new(ui)
        .id_salt(salt)
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .auto_shrink([false, false])
        .max_scroll_height(f32::INFINITY);
    // The last column takes whatever width is left, so it is never pushed off-screen when the
    // saved widths add up to more than the window.
    let last = p.columns.len().saturating_sub(1);
    for (i, col) in p.columns.iter().enumerate() {
        let column = if i == last {
            Column::remainder().at_least(60.0)
        } else {
            Column::initial(col.width.max(40.0)).at_least(40.0)
        };
        builder = builder.column(column.clip(true));
    }
    if let Some(row) = p.scroll_to {
        builder = builder.scroll_to_row(row, None);
    }
    let table = builder.header(HEADER_HEIGHT, |mut header| {
        for col in p.columns {
            let (_, resp) = header.col(|ui| header_cell(ui, col.id, p.sort));
            if resp.clicked() {
                out.header_click = Some(col.id);
            }
        }
    });
    table.body(|body| {
        // The last column is `remainder`: its width follows the window and the preview pane, so
        // it says nothing about what the user chose and must not be written back to the config
        // (that would save the config on every pane or window resize).
        out.widths = p
            .columns
            .iter()
            .zip(body.widths())
            .take(p.columns.len().saturating_sub(1))
            .map(|(c, w)| (c.id, *w))
            .collect();
        body.rows(ROW_HEIGHT, p.view.len(), |mut row| {
            let idx = row.index();
            row.set_selected(p.selection.contains(idx));
            let cached = cache.get(idx, p.size_format, p.date_format, || {
                build_row(p.view, idx, p.backend, p.size_format, p.date_format)
            });
            let editing_row = rename.as_ref().is_some_and(|e| e.row == idx);
            // Rename is for one item on an online volume; right-clicking an unselected row
            // selects just that row first.
            let renamable = !cached.offline && (p.selection.len() <= 1 || !p.selection.contains(idx));
            for col in p.columns {
                if editing_row && col.id == ColumnId::Name {
                    if let Some(edit) = rename.as_mut() {
                        row.col(|ui| {
                            icons::icon(ui, cached.category, cached.offline);
                            out.editor = rename_editor(ui, edit);
                            out.editor_drawn = true;
                        });
                    }
                    continue;
                }
                let (_, resp) = row.col(|ui| cell_content(ui, col.id, cached));
                let mods: Modifiers = resp.ctx.input(|i| i.modifiers);
                if resp.double_clicked() {
                    out.cell_events.push((idx, col.id, CellEvent::DoubleClick(mods)));
                } else if resp.clicked() {
                    out.cell_events.push((idx, col.id, CellEvent::Click(mods)));
                }
                if resp.secondary_clicked() {
                    out.cell_events.push((idx, col.id, CellEvent::Secondary));
                }
                resp.context_menu(|ui| context_menu(ui, idx, renamable, &mut out.context));
            }
        });
    });
    out
}

impl App {
    /// The central panel: header plus virtualised rows.
    pub(crate) fn results_table(&mut self, ui: &mut Ui) {
        let Some(view) = self.results.clone() else {
            ui.centered_and_justified(|ui| {
                ui.weak("Loading…");
            });
            return;
        };
        let columns: Vec<ColumnConfig> = self
            .config
            .ui
            .columns
            .iter()
            .filter(|c| c.visible)
            .cloned()
            .collect();
        let row_h = ROW_HEIGHT + ui.spacing().item_spacing.y;
        self.visible_rows = (((ui.available_height() - HEADER_HEIGHT) / row_h).floor() as usize).max(1);

        let params = TableParams {
            view: &view,
            columns: &columns,
            selection: &self.selection,
            sort: self.config.ui.sort,
            size_format: self.config.ui.size_format,
            date_format: &self.config.ui.date_format,
            scroll_to: self.scroll_to_row.take(),
            backend: &*self.backend,
        };
        let out = egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                draw_table(ui, &params, &mut self.cache, &mut self.rename)
            })
            .inner;

        let dragging = ui.input(|i| i.pointer.any_down());
        self.update_column_widths(&out.widths, dragging);
        if let Some(column) = out.header_click {
            self.sort_by(column);
        }
        // The editor's outcome first: a click elsewhere both ends the edit and selects.
        match out.editor {
            Some(EditorEvent::Commit(trigger)) => self.commit_rename(trigger),
            Some(EditorEvent::Cancel) => self.cancel_rename(),
            None if !out.editor_drawn && self.rename.as_ref().is_some_and(|e| e.shown) => {
                self.commit_rename(crate::app::CommitTrigger::Hidden);
            }
            None => {}
        }
        for (row, column, event) in out.cell_events {
            let effects = self.on_cell_event(row, column, event);
            self.apply_effects(effects);
        }
        for (row, action) in out.context {
            let effects = self.context_effects(row, action);
            self.apply_effects(effects);
        }
        if view.len() == 0 {
            let rect = ui.max_rect();
            ui.painter().text(
                rect.center() + vec2(0.0, HEADER_HEIGHT),
                egui::Align2::CENTER_CENTER,
                "No matching files",
                egui::FontId::proportional(14.0),
                ui.visuals().weak_text_color(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::TestApp;

    fn view_and_backend() -> (ResultsView, crate::backend::fake::FakeBackend) {
        let fx = crate::testkit::Fixture::new();
        let view = crate::testkit::results_of(&fx.snapshot, &[fx.report, fx.docs_entry]);
        let (fake, _handle, _rx) = crate::backend::fake::FakeBackend::new();
        (view, fake)
    }

    #[test]
    fn rows_are_formatted_for_display() {
        let (view, backend) = view_and_backend();
        let file = build_row(&view, 0, &backend, SizeFormat::Binary, "%Y-%m-%d");
        assert_eq!(file.name, "report.pdf");
        assert_eq!(file.path, "/home/me/docs");
        assert_eq!(file.kind, "PDF file");
        assert_eq!(file.size, "2.0 KiB");
        assert_eq!(file.category, Category::Document);
        assert!(!file.offline);
        assert!(file.modified.starts_with("2023-11-1"), "{}", file.modified);

        let folder = build_row(&view, 1, &backend, SizeFormat::Bytes, "%Y");
        assert_eq!(folder.kind, "Folder");
        assert_eq!(folder.category, Category::Folder);
        assert_eq!(folder.size, "2,058 B", "folders show their recursive total");
        assert_eq!(folder.path, "/home/me");
    }

    #[test]
    fn cache_is_invalidated_by_format_changes_and_new_results() {
        let (view, backend) = view_and_backend();
        let mut cache = RowCache::default();
        let mut builds = 0;
        let mut get = |cache: &mut RowCache, fmt: SizeFormat| {
            cache
                .get(0, fmt, "%Y", || {
                    builds += 1;
                    build_row(&view, 0, &backend, fmt, "%Y")
                })
                .size
                .clone()
        };
        assert_eq!(get(&mut cache, SizeFormat::Binary), "2.0 KiB");
        assert_eq!(get(&mut cache, SizeFormat::Binary), "2.0 KiB");
        assert_eq!(get(&mut cache, SizeFormat::Si), "2.0 kB");
        cache.invalidate();
        assert_eq!(get(&mut cache, SizeFormat::Si), "2.0 kB");
        assert_eq!(builds, 3, "second Binary lookup was a cache hit");
    }

    #[test]
    fn cache_stays_bounded() {
        let mut cache = RowCache::default();
        for row in 0..(MAX_CACHED_ROWS * 2) {
            let _ = cache.get(row, SizeFormat::Binary, "%Y", || CachedRow {
                name: String::new(),
                path: String::new(),
                kind: String::new(),
                modified: String::new(),
                size: String::new(),
                category: Category::Other,
                offline: false,
            });
        }
        assert!(cache.len() <= MAX_CACHED_ROWS);
    }

    #[test]
    fn offline_volumes_are_marked() {
        let mut t = TestApp::new();
        let mut snap = (*t.fixture.snapshot).clone();
        snap.volumes[0].state = VolumeState::Offline;
        let snap = std::sync::Arc::new(snap);
        let view = crate::testkit::results_of(&snap, &[t.fixture.report]);
        let row = build_row(&view, 0, &*t.app.backend, SizeFormat::Binary, "%Y");
        assert!(row.offline);
        t.app.cache.invalidate();
    }
}

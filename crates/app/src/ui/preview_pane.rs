//! The optional preview pane on the right.
//!
//! The pane has a fixed width taken from the config (View → Preview pane). Nothing it shows may
//! change that width: every content kind is laid out inside it, truncating, scaling or scrolling
//! what does not fit.

use std::ops::RangeInclusive;
use std::time::Duration;

use egui::containers::panel::PanelState;
use egui::{Align, Label, Layout, RichText, Ui, Vec2};
use everythux_preview::{MediaMeta, PreviewContent};

use crate::app::{App, Effect, PopupId};
use crate::format::{format_local, format_size};
use crate::keys::describe_shortcut;
use crate::preview::PreviewShown;

/// Narrowest and widest pane in logical pixels.
const PANE_WIDTH_RANGE: RangeInclusive<f32> = 160.0..=1200.0;
/// Width the result table keeps when the window is too narrow for the pane.
const TABLE_MIN_WIDTH: f32 = 300.0;
/// Width of the key column of the media metadata.
const META_KEY_WIDTH: f32 = 80.0;
/// Range of the "Custom…" width editor.
pub(crate) const CUSTOM_WIDTH_RANGE: RangeInclusive<f32> = 200.0..=1200.0;
/// The width presets of View → Preview pane.
pub(crate) const WIDTH_PRESETS: [(&str, u16); 4] = [
    ("Narrow", 280),
    ("Medium", 360),
    ("Wide", 520),
    ("Extra wide", 720),
];

fn panel_id() -> egui::Id {
    egui::Id::new("everythux_preview")
}

/// The width the pane is shown at: the configured width, unless the window is too narrow to keep
/// [`TABLE_MIN_WIDTH`] for the table. Then the pane gives way for this frame only.
pub(crate) fn shown_width(configured: f32, available: f32) -> f32 {
    let configured = if configured.is_finite() { configured } else { 360.0 };
    let wanted = configured.clamp(*PANE_WIDTH_RANGE.start(), *PANE_WIDTH_RANGE.end());
    let room = (available - TABLE_MIN_WIDTH).max(*PANE_WIDTH_RANGE.start());
    wanted.min(room).min(available).max(0.0)
}

/// `size` scaled down (never up) to `max_width`, keeping the aspect ratio.
fn fit_width(size: Vec2, max_width: f32) -> Vec2 {
    if size.x <= max_width || size.x <= 0.0 {
        size
    } else {
        size * (max_width / size.x)
    }
}

fn format_media_duration(d: Duration) -> String {
    let total = d.as_secs();
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn meta_grid(ui: &mut Ui, meta: &MediaMeta) {
    let mut rows: Vec<(String, String)> = Vec::new();
    if let Some(v) = &meta.title {
        rows.push(("Title".to_owned(), v.clone()));
    }
    if let Some(v) = &meta.artist {
        rows.push(("Artist".to_owned(), v.clone()));
    }
    if let Some(v) = &meta.album {
        rows.push(("Album".to_owned(), v.clone()));
    }
    if let Some(d) = meta.duration {
        rows.push(("Duration".to_owned(), format_media_duration(d)));
    }
    rows.extend(meta.extra.iter().cloned());
    if rows.is_empty() {
        ui.weak("No metadata");
        return;
    }
    let row_height = ui.spacing().interact_size.y;
    for (key, value) in rows {
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(META_KEY_WIDTH, row_height),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_min_width(META_KEY_WIDTH);
                    ui.add(Label::new(RichText::new(key).weak()).truncate());
                },
            );
            // Truncated labels show the full value as a tooltip.
            ui.add(Label::new(value).truncate());
        });
    }
}

/// Zoom range of an image, relative to its fitted size.
const ZOOM_RANGE: RangeInclusive<f32> = 1.0..=16.0;
/// Factor of the − and + buttons.
const ZOOM_STEP: f32 = 1.5;

/// `zoom` limited to [`ZOOM_RANGE`]; anything not finite means "fit".
fn clamp_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() {
        zoom.clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end())
    } else {
        1.0
    }
}

/// Scroll offset that keeps the image point under `pivot` (a position inside the viewport)
/// in place when the zoom changes from `from` to `to`.
fn zoomed_offset(offset: Vec2, pivot: Vec2, from: f32, to: f32) -> Vec2 {
    if from <= 0.0 {
        return Vec2::ZERO;
    }
    let scaled = (offset + pivot) * (to / from) - pivot;
    Vec2::new(scaled.x.max(0.0), scaled.y.max(0.0))
}

/// What the zoomed image remembers between frames.
#[derive(Debug, Clone, Copy)]
struct ZoomState {
    zoom: f32,
    offset: Vec2,
}

/// The −, + and Fit buttons and the percentage. Returns whether a button changed `zoom`.
fn zoom_buttons(ui: &mut Ui, zoom: &mut f32, percent: f32) -> bool {
    let before = *zoom;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(*zoom > *ZOOM_RANGE.start(), egui::Button::new("−").small())
            .on_hover_text("Zoom out (Ctrl+wheel)")
            .clicked()
        {
            *zoom = clamp_zoom(*zoom / ZOOM_STEP);
        }
        if ui
            .add_enabled(*zoom < *ZOOM_RANGE.end(), egui::Button::new("+").small())
            .on_hover_text("Zoom in (Ctrl+wheel)")
            .clicked()
        {
            *zoom = clamp_zoom(*zoom * ZOOM_STEP);
        }
        if ui
            .add_enabled(*zoom > 1.0, egui::Button::new("Fit").small())
            .on_hover_text("Fit to the pane (double-click)")
            .clicked()
        {
            *zoom = 1.0;
        }
        ui.weak(format!("{percent:.0}%"));
    });
    (*zoom - before).abs() > f32::EPSILON
}

/// An image fitted to the pane width. `Ctrl`+wheel (or a pinch) over it, or the buttons below it,
/// zoom in up to 16× around the pointer; the zoomed image is dragged or scrolled inside a frame
/// that keeps the fitted size, so the pane never grows. Double-click or "Fit" resets. The zoom is
/// remembered per file. While zoomed, the full-resolution render is shown once it has loaded.
fn show_image(ui: &mut Ui, shown: &PreviewShown, index: usize, original: Option<(u32, u32)>) {
    let Some(fit_texture) = shown.textures.get(index) else {
        return;
    };
    let fitted = fit_width(fit_texture.size_vec2(), ui.available_width());
    let id = egui::Id::new(("preview_zoom", &shown.path, index));
    let mut state = ui.data(|d| d.get_temp::<(f32, Vec2)>(id)).map_or(
        ZoomState {
            zoom: 1.0,
            offset: Vec2::ZERO,
        },
        |(zoom, offset)| ZoomState {
            zoom: clamp_zoom(zoom),
            offset,
        },
    );
    let before = state.zoom;
    let zoomed = state.zoom > 1.0;
    // The sharp render replaces the screen-sized one only while zoomed.
    let texture = match (&shown.hires, zoomed && index == 0) {
        (Some(hires), true) => hires,
        _ => fit_texture,
    };
    if zoomed && index == 0 {
        shown.wants_hires.set(true);
    }
    let image = |size: Vec2| {
        egui::Image::from_texture(egui::load::SizedTexture::from_handle(texture))
            .fit_to_exact_size(size)
            .sense(egui::Sense::click())
    };

    let mut viewport = None;
    let response = if zoomed {
        let area = egui::ScrollArea::both()
            .id_salt(id)
            .max_height(fitted.y)
            .min_scrolled_height(fitted.y)
            .auto_shrink([false, false])
            .scroll_offset(state.offset)
            .show(ui, |ui| ui.add(image(fitted * state.zoom)));
        state.offset = area.state.offset;
        viewport = Some(area.inner_rect);
        area.inner
    } else {
        ui.add(image(fitted))
    };

    // Zoom around the pointer, or the viewport centre when it is outside.
    let pivot_of = |rect: egui::Rect, pointer: Option<egui::Pos2>| {
        pointer
            .filter(|p| rect.contains(*p))
            .unwrap_or_else(|| rect.center())
            - rect.min
    };
    let pointer = ui.input(|i| i.pointer.hover_pos());
    if response.hovered() {
        let wheel = ui.input(egui::InputState::zoom_delta);
        if (wheel - 1.0).abs() > f32::EPSILON {
            state.zoom = clamp_zoom(state.zoom * wheel);
        }
    }
    if response.double_clicked() {
        state.zoom = 1.0;
    }
    let full_width = original.map_or(fit_texture.size_vec2().x, |(w, _)| w as f32);
    let percent = if full_width > 0.0 {
        fitted.x * state.zoom / full_width * 100.0
    } else {
        100.0
    };
    let mut pivot_from = pointer;
    if zoom_buttons(ui, &mut state.zoom, percent) {
        pivot_from = None;
    }
    if (state.zoom - before).abs() > f32::EPSILON {
        state.offset = if state.zoom <= 1.0 {
            Vec2::ZERO
        } else {
            // The first zoom step starts from the unscrolled image, whose viewport is the
            // image's own rect.
            let rect = viewport.unwrap_or(response.rect);
            zoomed_offset(state.offset, pivot_of(rect, pivot_from), before, state.zoom)
        };
        ui.ctx().request_repaint();
    }
    ui.data_mut(|d| d.insert_temp(id, (state.zoom, state.offset)));
}

fn show_content(ui: &mut Ui, shown: &PreviewShown) {
    match &shown.content {
        PreviewContent::Image { original_size, .. } => {
            show_image(ui, shown, 0, *original_size);
            if let Some((w, h)) = original_size {
                ui.weak(format!("{w} × {h} px"));
            }
        }
        PreviewContent::Pdf { pages, .. } => {
            show_image(ui, shown, 0, None);
            if let Some(pages) = pages {
                ui.weak(format!("{pages} page(s), showing page 1"));
            }
        }
        // Drawn by `App::text_preview` from `PreviewShown::text`.
        PreviewContent::Text { .. } => {}
        PreviewContent::Audio { meta, .. } | PreviewContent::Video { meta, .. } => {
            show_image(ui, shown, 0, None);
            ui.add_space(4.0);
            meta_grid(ui, meta);
        }
        PreviewContent::Folder { children } => {
            match children {
                Some(n) => ui.label(format!("Folder with {n} item(s)")),
                None => ui.weak("Folder"),
            };
        }
        PreviewContent::Unavailable { hint } => {
            ui.add(Label::new(RichText::new(hint).weak()).wrap());
        }
        _ => {
            ui.weak("No preview available");
        }
    }
}

impl App {
    pub(crate) fn show_preview_panel(&mut self, ui: &mut Ui) {
        let pane = &self.config.ui.preview_pane;
        let width = shown_width(pane.width, ui.available_width());
        let resizable = pane.resizable;
        let pointer_down = ui.input(|i| i.pointer.any_down());
        let panel = egui::Panel::right(panel_id());
        let panel = if resizable {
            // egui remembers the size of the last drag. A width chosen in the menu (or a window
            // that grew back) wins over it whenever no drag is in progress.
            let remembered = PanelState::load(ui.ctx(), panel_id()).map(|s| s.size().x);
            if !pointer_down
                && !self.preview.width_dirty
                && remembered.is_some_and(|w| (w - width).abs() > 0.5)
            {
                ui.ctx().data_mut(|d| d.remove::<PanelState>(panel_id()));
            }
            panel
                .resizable(true)
                .default_size(width)
                .size_range(PANE_WIDTH_RANGE)
        } else {
            panel.resizable(false).exact_size(width)
        };
        let response = panel.show(ui, |ui| {
            // A panel is only as wide as its content. Without this the pane would shrink to
            // whatever the current preview needs (and to the minimum while empty).
            ui.take_available_width();
            self.preview_pane(ui);
            ui.min_rect()
        });
        self.preview.pane_rect = Some(response.response.rect);
        self.preview.content_rect = Some(response.inner);
        if resizable {
            self.track_preview_width(response.response.rect.width(), pointer_down);
        }
    }

    /// Follows a user resize of the pane and persists the final width once, on release.
    ///
    /// The width only changes while the pointer is down: a smaller window squeezes the panel
    /// without the user having asked for a narrower pane, and that must not overwrite the saved
    /// preference. While dragging nothing is written; the release frame saves once.
    fn track_preview_width(&mut self, panel_width: f32, pointer_down: bool) {
        let pane = &mut self.config.ui.preview_pane;
        if pointer_down && (panel_width - pane.width).abs() > 0.5 {
            pane.width = panel_width;
            self.preview.width_dirty = true;
        }
        if !pointer_down && std::mem::take(&mut self.preview.width_dirty) {
            self.config_changed();
        }
    }

    fn preview_pane(&mut self, ui: &mut Ui) {
        // Every branch below lays out inside exactly this width.
        let width = ui.available_width();
        ui.set_max_width(width);
        ui.set_min_width(width);
        let Some(view) = self.results.clone() else {
            return;
        };
        let Some(row) = self.selection.cursor().filter(|r| *r < view.len()) else {
            ui.add_space(8.0);
            ui.weak("Select a file to preview it.");
            return;
        };
        let snap = &*view.snapshot;
        let Some(id) = view.entry(row) else { return };
        let i = id.index();

        ui.add_space(4.0);
        // Truncated labels show the full text as a tooltip.
        ui.add(Label::new(RichText::new(String::from_utf8_lossy(snap.name(id))).strong()).truncate());
        ui.add(
            Label::new(
                RichText::new(format!(
                    "{} · {} · {}",
                    format_size(snap.entries.size[i], self.config.ui.size_format),
                    format_local(snap.entries.mtime_ns[i], &self.config.ui.date_format),
                    snap.parent_path(id).to_string_lossy()
                ))
                .weak(),
            )
            .truncate(),
        );
        if ui.button("Open").clicked() {
            let effects = vec![Effect::OpenDefault(snap.full_path(id))];
            self.apply_effects(effects);
        }
        ui.separator();

        let current = self
            .preview
            .shown
            .as_ref()
            .filter(|s| Some(&s.path) == self.preview.wanted.as_ref());
        match current {
            Some(shown) if shown.text.is_some() => self.text_preview(ui),
            Some(shown) => show_content(ui, shown),
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Loading preview…");
                });
            }
        }
    }

    /// The "Custom…" width window: a live editor for `ui.preview_pane.width`.
    pub(crate) fn preview_width_window(&mut self, ctx: &egui::Context) {
        if !self.preview.width_window {
            return;
        }
        let mut open = true;
        let mut width = self.config.ui.preview_pane.width.round();
        let shown = self.preview.pane_rect.map(|r| r.width().round());
        egui::Window::new("Preview pane width")
            .id(egui::Id::new("everythux_preview_width"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Width:");
                    let drag = egui::DragValue::new(&mut width)
                        .range(CUSTOM_WIDTH_RANGE)
                        .speed(10.0)
                        .fixed_decimals(0)
                        .suffix(" px");
                    if ui.add(drag).changed() {
                        // Steps of 10 px, whether dragged or typed.
                        width = ((width / 10.0).round() * 10.0)
                            .clamp(*CUSTOM_WIDTH_RANGE.start(), *CUSTOM_WIDTH_RANGE.end());
                    }
                });
                match shown {
                    Some(shown) if (shown - width).abs() > 1.0 && self.config.ui.preview_pane.enabled => {
                        ui.weak(format!(
                            "The window is too narrow: the pane is shown at {shown} px for now."
                        ));
                    }
                    _ if !self.config.ui.preview_pane.enabled => {
                        ui.weak(format!(
                            "The preview pane is hidden ({} shows it).",
                            describe_shortcut(self.bindings.toggle_preview)
                        ));
                    }
                    _ => {
                        ui.weak("Changes apply and are saved immediately.");
                    }
                }
            });
        self.set_preview_width(width);
        if !open {
            self.close_popup(PopupId::PreviewWidth);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_width_keeps_the_configured_width_while_it_fits() {
        assert!((shown_width(360.0, 1200.0) - 360.0).abs() < f32::EPSILON);
        assert!(
            (shown_width(5000.0, 4000.0) - 1200.0).abs() < f32::EPSILON,
            "clamped to the maximum"
        );
        assert!((shown_width(f32::NAN, 1200.0) - 360.0).abs() < f32::EPSILON);
    }

    #[test]
    fn shown_width_gives_way_in_narrow_windows() {
        assert!(
            (shown_width(360.0, 600.0) - 300.0).abs() < f32::EPSILON,
            "table keeps 300 px"
        );
        assert!(
            (shown_width(360.0, 400.0) - 160.0).abs() < f32::EPSILON,
            "never below the minimum"
        );
        assert!(
            (shown_width(360.0, 100.0) - 100.0).abs() < f32::EPSILON,
            "never wider than the window"
        );
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pivot() {
        // Pivot at the viewport centre of an unscrolled image: the image point (50, 40) stays put.
        let off = zoomed_offset(Vec2::ZERO, Vec2::new(50.0, 40.0), 1.0, 2.0);
        assert_eq!(off, Vec2::new(50.0, 40.0));
        // Back to 1x returns to the origin.
        assert_eq!(zoomed_offset(off, Vec2::new(50.0, 40.0), 2.0, 1.0), Vec2::ZERO);
    }

    #[test]
    fn zoom_is_kept_in_range() {
        assert!((clamp_zoom(0.2) - 1.0).abs() < f32::EPSILON);
        assert!((clamp_zoom(100.0) - 16.0).abs() < f32::EPSILON);
        assert!((clamp_zoom(f32::NAN) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn images_scale_down_to_the_width_but_never_up() {
        assert_eq!(fit_width(Vec2::new(4000.0, 400.0), 400.0), Vec2::new(400.0, 40.0));
        assert_eq!(fit_width(Vec2::new(100.0, 50.0), 400.0), Vec2::new(100.0, 50.0));
    }
}

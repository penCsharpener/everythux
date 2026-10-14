//! The optional preview pane on the right.

use std::time::Duration;

use egui::{RichText, ScrollArea, Ui};
use everythux_preview::{MediaMeta, PreviewContent};

use crate::app::{App, Effect};
use crate::format::{format_local, format_size};
use crate::preview::PreviewShown;

/// Allowed pane width in logical pixels.
const PANE_WIDTH_RANGE: std::ops::RangeInclusive<f32> = 160.0..=1000.0;

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
    egui::Grid::new("preview_meta")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            for (key, value) in rows {
                ui.weak(key);
                ui.add(egui::Label::new(value).wrap());
                ui.end_row();
            }
        });
}

fn show_image(ui: &mut Ui, shown: &PreviewShown, index: usize) {
    if let Some(texture) = shown.textures.get(index) {
        ui.add(egui::Image::from_texture(egui::load::SizedTexture::from_handle(texture)).shrink_to_fit());
    }
}

fn show_content(ui: &mut Ui, shown: &PreviewShown) {
    match &shown.content {
        PreviewContent::Image { original_size, .. } => {
            show_image(ui, shown, 0);
            if let Some((w, h)) = original_size {
                ui.weak(format!("{w} × {h} px"));
            }
        }
        PreviewContent::Pdf { pages, .. } => {
            show_image(ui, shown, 0);
            if let Some(pages) = pages {
                ui.weak(format!("{pages} page(s), showing page 1"));
            }
        }
        PreviewContent::Text { text, truncated } => {
            ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(text).monospace()).selectable(true));
                if *truncated {
                    ui.weak("… (truncated)");
                }
            });
        }
        PreviewContent::Audio { meta, .. } | PreviewContent::Video { meta, .. } => {
            show_image(ui, shown, 0);
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
            ui.weak(hint);
        }
        _ => {
            ui.weak("No preview available");
        }
    }
}

impl App {
    pub(crate) fn show_preview_panel(&mut self, ui: &mut Ui) {
        let width = self.config.ui.preview_pane.width;
        let response = egui::Panel::right("everythux_preview")
            .resizable(true)
            .default_size(width)
            .size_range(PANE_WIDTH_RANGE)
            .show(ui, |ui| {
                // A panel is only as wide as its content. Without this the pane would shrink to
                // whatever the current preview needs (and to the minimum while empty), changing
                // the layout on every selection change instead of staying where the user put it.
                ui.take_available_width();
                self.preview_pane(ui);
            });
        self.track_preview_width(response.response.rect.width(), ui.input(|i| i.pointer.any_down()));
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
        ui.add(egui::Label::new(RichText::new(String::from_utf8_lossy(snap.name(id))).strong()).wrap());
        ui.weak(format!(
            "{} · {}",
            format_size(snap.entries.size[i], self.config.ui.size_format),
            format_local(snap.entries.mtime_ns[i], &self.config.ui.date_format)
        ));
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
            Some(shown) => show_content(ui, shown),
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Loading preview…");
                });
            }
        }
    }
}

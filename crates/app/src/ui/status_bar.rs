//! The status bar at the bottom of the window.

use egui::{RichText, Ui};

use crate::app::{App, Command};
use crate::format::{format_count, format_duration};
use crate::status::ToastKind;

impl App {
    /// "N objects" / "N of M objects".
    pub(crate) fn object_count_text(&self) -> String {
        match &self.results {
            None => "Loading…".to_owned(),
            Some(r) => {
                let (n, total) = (r.len() as u64, r.total_entries as u64);
                if n == total {
                    format!("{} objects", format_count(total))
                } else {
                    format!("{} of {} objects", format_count(n), format_count(total))
                }
            }
        }
    }

    pub(crate) fn status_bar(&mut self, ui: &mut Ui) {
        let mut command = None;
        let mut undo = None;
        ui.horizontal(|ui| {
            ui.label(self.object_count_text());
            if let Some(r) = &self.results {
                ui.separator();
                ui.weak(format_duration(r.elapsed));
            }
            // With several bars, spell out what is being searched for.
            if self.search.bars.len() > 1 {
                let query = self.search.combined_query();
                if !query.is_empty() {
                    ui.separator();
                    ui.weak(query);
                }
            }
            let index_text = self.index.text(self.index_is_empty());
            if !index_text.is_empty() {
                ui.separator();
                let failed = matches!(
                    self.index.state,
                    crate::status::IndexState::Done { failed: true, .. }
                );
                if failed {
                    ui.colored_label(ui.visuals().error_fg_color, &index_text);
                } else {
                    ui.label(&index_text);
                }
            }
            if self.index.is_running() && ui.small_button("Cancel").clicked() {
                command = Some(Command::CancelJob);
            }
            if let Some(toast) = self.toasts.last() {
                ui.separator();
                let text = RichText::new(&toast.text);
                match toast.kind {
                    ToastKind::Info => ui.label(text),
                    ToastKind::Error => ui.label(text.color(ui.visuals().error_fg_color)),
                };
                if toast.undo.is_some() && ui.small_button("Undo").clicked() {
                    undo.clone_from(&toast.undo);
                }
            }
        });
        if let Some(command) = command {
            self.run_command(command);
        }
        if let Some(undo) = undo {
            self.undo_toast(undo);
        }
    }
}

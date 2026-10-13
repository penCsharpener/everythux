//! The search box and its error line.

use egui::{Key, TextEdit, Ui};

use crate::app::App;
use crate::app::search_id;

impl App {
    pub(crate) fn search_bar(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        let response = ui.add(
            TextEdit::singleline(&mut self.search.text)
                .id(search_id())
                .hint_text("Search files and folders")
                .desired_width(f32::INFINITY),
        );
        if self.search.focus_requested {
            response.request_focus();
            self.search.focus_requested = false;
        }
        if response.changed() {
            self.resubmit_search();
        }
        // Enter surrenders focus in a single-line editor; Enter is consumed earlier as "open"
        // so this only guards against the editor losing focus for other reasons.
        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            response.request_focus();
        }
        if let Some(error) = &self.search.error {
            let muted = ui.visuals().error_fg_color.gamma_multiply(0.8);
            ui.label(egui::RichText::new(error).small().color(muted));
        }
        ui.add_space(2.0);
    }
}

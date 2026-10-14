//! The tab strip: one selectable label per tab with a `×`, and a `+` that opens a new tab.

use egui::Ui;

use crate::app::tabs_limit;
use crate::app::{App, Command};
use crate::keys::describe_shortcut;
use crate::ui::icons::{Glyph, glyph_button};

impl App {
    pub(crate) fn tab_bar(&mut self, ui: &mut Ui) {
        let new_tip = format!("New tab · {}", describe_shortcut(self.bindings.new_tab));
        let close_tip = format!("Close tab · {}", describe_shortcut(self.bindings.close_tab));
        let several = self.tab_count() > 1;
        let can_add = self.tab_count() < tabs_limit();
        let mut command = None;
        ui.horizontal(|ui| {
            for index in 0..self.tab_count() {
                let label = self.tab_title(index);
                let selected = index == self.active_tab;
                let response = ui.selectable_label(selected, label);
                if response.clicked() {
                    command = Some(Command::SelectTab(index));
                }
                if response.middle_clicked() {
                    command = Some(Command::CloseTabAt(index));
                }
                if several
                    && glyph_button(ui, Glyph::Cross, "Close tab", true)
                        .on_hover_text(&close_tip)
                        .clicked()
                {
                    command = Some(Command::CloseTabAt(index));
                }
                ui.add_space(4.0);
            }
            let plus = glyph_button(ui, Glyph::Plus, "New tab", can_add)
                .on_hover_text(&new_tip)
                .on_disabled_hover_text("Too many tabs");
            if plus.clicked() {
                command = Some(Command::NewTab);
            }
        });
        if let Some(command) = command {
            self.run_command(command);
        }
    }
}

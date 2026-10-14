//! One UI frame: input handling before any widget exists, then panels, then windows.

use egui::{Key, Modifiers};

use super::interact::{CopyKind, Nav};
use super::{App, Command};

/// Widget id of the search box, shared with the input handling.
pub(crate) fn search_id() -> egui::Id {
    egui::Id::new("everythux_search_box")
}

impl App {
    /// Renders the whole application into `ui` (the root ui of the viewport).
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // Normally the same context as at construction; adopting the live one keeps clipboard,
        // repaint and viewport requests pointed at the context that is actually running.
        self.ctx = ctx.clone();
        self.now = ctx.input(|i| i.time);
        self.pointer = ctx.input(|i| i.pointer.latest_pos()).or(self.pointer);
        self.poll_events();
        self.expire_toasts();
        self.tick_slow_click();

        // Consume shortcuts before any widget exists, so the focused search box cannot swallow them.
        for command in self.read_input_commands(&ctx) {
            self.run_command(command);
        }

        egui::Panel::top("everythux_menu").show(ui, |ui| self.menu_bar(ui));
        self.show_banners(ui);
        egui::Panel::top("everythux_search").show(ui, |ui| self.search_bar(ui));
        egui::Panel::bottom("everythux_status").show(ui, |ui| self.status_bar(ui));
        if self.config.ui.preview_pane.enabled {
            self.show_preview_panel(ui);
        }
        egui::CentralPanel::default().show(ui, |ui| self.results_table(ui));

        self.show_windows(&ctx);
        self.sync_popups();
        self.drive_preview(&ctx);
    }

    /// Reads global shortcuts and table keys from the input of this frame and consumes them.
    pub(crate) fn read_input_commands(&self, ctx: &egui::Context) -> Vec<Command> {
        let text_focused = ctx.egui_wants_keyboard_input();
        let search_focused = ctx.memory(|m| m.has_focus(search_id()));
        let popup_open = ctx.any_popup_open() || egui::Popup::is_any_open(ctx);
        let has_selection = !self.selection.is_empty();
        // While the rename editor is open, typed keys belong to it: only Ctrl+Shift+R and Ctrl+Q
        // stay global, and the editor consumes Enter, Tab and Escape itself.
        let editing = self.rename.is_some();
        let b = &self.bindings;

        ctx.input_mut(|i| {
            let mut commands = Vec::new();
            // Most specific shortcuts first: egui ignores extra Shift, so Ctrl+Shift+R must be
            // tested before a plain Ctrl+R binding could match it.
            let globals = [
                (&b.reindex_all, Command::FullReindex),
                (&b.quick_rescan, Command::QuickRescan),
                (&b.toggle_preview, Command::TogglePreview),
                (&b.focus_search, Command::FocusSearch),
                (&b.toggle_match_path, Command::ToggleMatchPath),
            ];
            // `reindex_all` is first: it is the only one that stays active while editing.
            let active = if editing { 1 } else { globals.len() };
            for (shortcut, command) in globals.into_iter().take(active) {
                if i.consume_shortcut(shortcut) {
                    commands.push(command);
                }
            }
            if i.consume_key(Modifiers::CTRL, Key::Q) {
                commands.push(Command::Quit);
            }
            if !editing && !popup_open && i.consume_key(Modifiers::NONE, Key::Escape) {
                commands.push(Command::Escape);
            }

            // Keys for the result table. Up/Down/Page and Enter also work while the search box
            // has focus (they have no meaning in a single-line editor); Home/End and the copy
            // shortcuts only when no text field is being edited.
            let table_keys = !editing && (!text_focused || search_focused);
            if !editing
                && !popup_open
                && (!text_focused || search_focused)
                && i.consume_key(Modifiers::NONE, Key::F2)
            {
                commands.push(Command::Rename);
            }
            if table_keys {
                if i.consume_key(Modifiers::CTRL, Key::Enter) {
                    commands.push(Command::ShowInFolder);
                } else if i.consume_key(Modifiers::NONE, Key::Enter) {
                    commands.push(Command::Open);
                }
                let extend = i.modifiers.shift;
                for (key, nav) in [
                    (Key::ArrowUp, Nav::Up),
                    (Key::ArrowDown, Nav::Down),
                    (Key::PageUp, Nav::PageUp),
                    (Key::PageDown, Nav::PageDown),
                ] {
                    while i.consume_key(Modifiers::NONE, key) {
                        commands.push(Command::Navigate(nav, extend));
                    }
                }
            }
            if !text_focused && !editing {
                let extend = i.modifiers.shift;
                for (key, nav) in [(Key::Home, Nav::Home), (Key::End, Nav::End)] {
                    if i.consume_key(Modifiers::NONE, key) {
                        commands.push(Command::Navigate(nav, extend));
                    }
                }
                if has_selection {
                    // Native backends turn Ctrl+C into `Event::Copy` and drop the key event, so the
                    // Shift state has to come from the modifier state.
                    let had_copy_event = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
                    if had_copy_event {
                        i.events.retain(|e| !matches!(e, egui::Event::Copy));
                        let kind = if i.modifiers.shift {
                            CopyKind::FolderPath
                        } else {
                            CopyKind::FullPath
                        };
                        commands.push(Command::Copy(kind));
                    } else if i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::C) {
                        commands.push(Command::Copy(CopyKind::FolderPath));
                    } else if i.consume_key(Modifiers::CTRL, Key::C) {
                        commands.push(Command::Copy(CopyKind::FullPath));
                    }
                }
            }
            commands
        })
    }

    fn show_banners(&mut self, ui: &mut egui::Ui) {
        if self.banners.is_empty() {
            return;
        }
        let mut dismissed = None;
        egui::Panel::top("everythux_banners").show(ui, |ui| {
            let color = ui.visuals().warn_fg_color;
            for (i, banner) in self.banners.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(color, format!("⚠ {}", banner.text));
                    if ui.small_button("Dismiss").clicked() {
                        dismissed = Some(i);
                    }
                });
            }
        });
        if let Some(i) = dismissed {
            self.banners.remove(i);
        }
    }
}

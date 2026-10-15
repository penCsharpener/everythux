//! One UI frame: input handling before any widget exists, then panels, then windows.

use egui::{Key, KeyboardShortcut, Modifiers};

use super::interact::{CopyKind, Nav};
use super::{App, Command};
use crate::keys::Bindings;
use crate::search_bars::FIRST_BAR;

/// Widget id of the first search bar, shared with the input handling.
pub(crate) fn search_id() -> egui::Id {
    egui::Id::new("everythux_search_box")
}

/// Widget id of search bar `bar` (by its stable id, so focus survives closing a bar above it).
pub(crate) fn bar_widget_id(bar: u32) -> egui::Id {
    if bar == FIRST_BAR {
        search_id()
    } else {
        egui::Id::new(("everythux_search_bar", bar))
    }
}

/// Consumes the search bar and tab shortcuts, appending the commands they stand for. Returns
/// whether one of them was a focus switch (the caller then cancels egui's own Tab traversal).
fn bar_shortcuts(
    i: &mut egui::InputState,
    b: &Bindings,
    bar_focused: bool,
    commands: &mut Vec<Command>,
) -> bool {
    let mut switched_focus = false;
    // The search bar shortcuts first: egui ignores an extra Shift, so the plain tab
    // shortcut (Ctrl+T) would also match the bar one (Ctrl+Shift+T).
    if i.consume_shortcut(&b.add_search_bar) {
        commands.push(Command::AddSearchBar);
    }
    // Consumed in the first bar too, where it does nothing: the text field would
    // otherwise take the key as "delete word", which is not what the binding promises.
    if i.consume_shortcut(&b.close_search_bar) {
        if bar_focused {
            commands.push(Command::CloseSearchBar);
        }
    } else if i.consume_shortcut(&b.close_tab) {
        commands.push(Command::CloseTab);
    }
    if i.consume_shortcut(&b.new_tab) {
        commands.push(Command::NewTab);
    }
    let next = b.next_tab;
    let previous = KeyboardShortcut::new(next.modifiers | Modifiers::SHIFT, next.logical_key);
    if i.consume_shortcut(&previous) {
        commands.push(Command::CycleTab(false));
        switched_focus = true;
    } else if i.consume_shortcut(&next) {
        commands.push(Command::CycleTab(true));
        switched_focus = true;
    }
    // Shift reverses the direction; with only two stops (list and newest bar) both
    // directions toggle. Test the Shift form first, as egui ignores an extra Shift.
    let switch = b.switch_focus;
    let reverse = KeyboardShortcut::new(switch.modifiers | Modifiers::SHIFT, switch.logical_key);
    if i.consume_shortcut(&reverse) || i.consume_shortcut(&switch) {
        commands.push(Command::SwitchFocus);
        switched_focus = true;
    }
    switched_focus
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
        egui::Panel::top("everythux_toolbar").show(ui, |ui| self.toolbar(ui));
        self.show_banners(ui);
        egui::Panel::top("everythux_tabs").show(ui, |ui| self.tab_bar(ui));
        egui::Panel::top("everythux_search").show(ui, |ui| self.search_bar(ui));
        egui::Panel::bottom("everythux_status").show(ui, |ui| self.status_bar(ui));
        if self.config.ui.preview_pane.enabled {
            self.show_preview_panel(ui);
        }
        egui::CentralPanel::default().show(ui, |ui| self.results_table(ui));

        self.show_windows(&ctx);
        self.sync_popups();
        self.drive_preview(&ctx);
        self.drive_pdf_page();
        self.drive_hires();
        self.sync_window_title();
    }

    /// Reads global shortcuts and table keys from the input of this frame and consumes them.
    pub(crate) fn read_input_commands(&self, ctx: &egui::Context) -> Vec<Command> {
        let text_focused = ctx.egui_wants_keyboard_input();
        let focused_bar = self.focused_bar_in(ctx, false);
        let search_focused = focused_bar.is_some();
        let popup_open = ctx.any_popup_open() || egui::Popup::is_any_open(ctx);
        // Ctrl+C copies selected preview text instead of paths (the text view handles it).
        let has_selection = !self.selection.is_empty() && !self.preview_text_selected();
        // While the rename editor is open, typed keys belong to it: only Ctrl+Shift+R and Ctrl+Q
        // stay global, and the editor consumes Enter, Tab and Escape itself.
        let editing = self.rename.is_some();
        // The search bar shortcuts work from the list and from any bar, but not while a text
        // field in a window (an exclusion rule, say) is being edited.
        let bar_keys = !editing && !popup_open && (!text_focused || focused_bar.is_some());
        let mut switched_focus = false;
        let b = &self.bindings;

        let commands = ctx.input_mut(|i| {
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
            if bar_keys {
                switched_focus = bar_shortcuts(i, b, focused_bar.is_some(), &mut commands);
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
        });
        if switched_focus {
            // egui turned the Tab into focus traversal when the frame began; cancel that, so Tab
            // never walks through the table cells.
            ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
        }
        commands
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

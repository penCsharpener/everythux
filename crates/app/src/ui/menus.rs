//! The menu bar.

use egui::{Button, Ui};
use everythux_core::config::{SizeFormat, Theme};

use crate::app::{App, Command};
use crate::keys::describe_shortcut;
use crate::ui::preview_pane::WIDTH_PRESETS;

fn item(ui: &mut Ui, label: &str, shortcut: &str, commands: &mut Vec<Command>, command: Command) {
    if ui.add(Button::new(label).shortcut_text(shortcut)).clicked() {
        commands.push(command);
        ui.close();
    }
}

fn toggle(ui: &mut Ui, label: &str, shortcut: &str, on: bool, commands: &mut Vec<Command>, command: Command) {
    if ui
        .add(Button::selectable(on, label).shortcut_text(shortcut))
        .clicked()
    {
        commands.push(command);
        ui.close();
    }
}

impl App {
    pub(crate) fn menu_bar(&mut self, ui: &mut Ui) {
        let mut commands: Vec<Command> = Vec::new();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                item(ui, "Quit", "Ctrl+Q", &mut commands, Command::Quit);
            });
            ui.menu_button("Search", |ui| self.search_menu(ui, &mut commands));
            ui.menu_button("Index", |ui| self.index_menu(ui, &mut commands));
            ui.menu_button("View", |ui| self.view_menu(ui, &mut commands));
            ui.menu_button("Help", |ui| {
                item(
                    ui,
                    "Keyboard shortcuts",
                    "",
                    &mut commands,
                    Command::ShowShortcuts,
                );
                item(ui, "About Everythux", "", &mut commands, Command::ShowAbout);
            });
        });
        for command in commands {
            self.run_command(command);
        }
    }

    fn search_menu(&self, ui: &mut Ui, commands: &mut Vec<Command>) {
        let s = &self.config.search;
        let b = &self.bindings;
        toggle(
            ui,
            "Match case",
            "",
            s.case_sensitive,
            commands,
            Command::ToggleCaseSensitive,
        );
        toggle(
            ui,
            "Match path",
            &describe_shortcut(b.toggle_match_path),
            s.match_path,
            commands,
            Command::ToggleMatchPath,
        );
        toggle(ui, "Regex", "", s.regex, commands, Command::ToggleRegex);
        toggle(
            ui,
            "Show only duplicates",
            "",
            self.duplicates_only,
            commands,
            Command::ToggleDuplicates,
        );
        ui.separator();
        item(
            ui,
            "Focus search box",
            &describe_shortcut(b.focus_search),
            commands,
            Command::FocusSearch,
        );
    }

    fn index_menu(&self, ui: &mut Ui, commands: &mut Vec<Command>) {
        let b = &self.bindings;
        item(
            ui,
            "Reindex all",
            &describe_shortcut(b.reindex_all),
            commands,
            Command::FullReindex,
        );
        item(
            ui,
            "Quick rescan",
            &describe_shortcut(b.quick_rescan),
            commands,
            Command::QuickRescan,
        );
        ui.add_enabled_ui(self.index.is_running(), |ui| {
            item(ui, "Cancel current job", "", commands, Command::CancelJob);
        });
        ui.separator();
        item(ui, "Volumes…", "", commands, Command::OpenVolumes);
        item(ui, "Exclusions…", "", commands, Command::OpenExclusions);
    }

    fn view_menu(&self, ui: &mut Ui, commands: &mut Vec<Command>) {
        let cfg = &self.config.ui;
        ui.menu_button("Preview pane", |ui| self.preview_pane_menu(ui, commands));
        ui.menu_button("Columns", |ui| {
            for col in &cfg.columns {
                let mut visible = col.visible;
                if ui.checkbox(&mut visible, col.id.label()).changed() {
                    commands.push(Command::ToggleColumn(col.id));
                }
            }
            ui.separator();
            let mut locked = cfg.lock_column_widths;
            if ui.checkbox(&mut locked, "Lock column widths").changed() {
                commands.push(Command::ToggleLockColumnWidths);
            }
        });
        ui.menu_button("Context menu", |ui| {
            let menu = &cfg.context_menu;
            let (mut open_with, mut service) = (menu.open_with, menu.service_menus);
            if ui.checkbox(&mut open_with, "Show \"Open with\"").changed() {
                commands.push(Command::ToggleContextOpenWith);
            }
            if ui
                .checkbox(&mut service, "Show actions of other programs")
                .on_hover_text("KDE service menus, Nemo actions and Thunar custom actions")
                .changed()
            {
                commands.push(Command::ToggleContextServiceMenus);
            }
            ui.separator();
            let hidden = menu.hidden.len();
            if ui
                .add_enabled(
                    hidden > 0,
                    Button::new(format!("Restore hidden entries ({hidden})")),
                )
                .clicked()
            {
                commands.push(Command::RestoreHiddenContextEntries);
                ui.close();
            }
        });
        ui.menu_button("Theme", |ui| {
            for (theme, label) in [
                (Theme::System, "System"),
                (Theme::Light, "Light"),
                (Theme::Dark, "Dark"),
            ] {
                if ui.radio(cfg.theme == theme, label).clicked() {
                    commands.push(Command::SetTheme(theme));
                    ui.close();
                }
            }
        });
        ui.menu_button("Size format", |ui| {
            for (format, label) in [
                (SizeFormat::Binary, "Binary (KiB, MiB)"),
                (SizeFormat::Si, "SI (kB, MB)"),
                (SizeFormat::Bytes, "Exact bytes"),
            ] {
                if ui.radio(cfg.size_format == format, label).clicked() {
                    commands.push(Command::SetSizeFormat(format));
                    ui.close();
                }
            }
        });
    }

    fn preview_pane_menu(&self, ui: &mut Ui, commands: &mut Vec<Command>) {
        let pane = &self.config.ui.preview_pane;
        toggle(
            ui,
            "Show preview pane",
            &describe_shortcut(self.bindings.toggle_preview),
            pane.enabled,
            commands,
            Command::TogglePreview,
        );
        ui.separator();
        let mut is_preset = false;
        for (label, width) in WIDTH_PRESETS {
            let on = (pane.width - f32::from(width)).abs() < 0.5;
            is_preset |= on;
            if ui
                .add(Button::selectable(on, format!("{label} ({width} px)")))
                .clicked()
            {
                commands.push(Command::SetPreviewWidth(width));
                ui.close();
            }
        }
        let custom = if is_preset {
            "Custom…".to_owned()
        } else {
            format!("Custom ({} px)…", pane.width.round())
        };
        if ui.add(Button::selectable(!is_preset, custom)).clicked() {
            commands.push(Command::CustomPreviewWidth);
            ui.close();
        }
        ui.separator();
        let mut resizable = pane.resizable;
        if ui
            .checkbox(&mut resizable, "Allow resizing by dragging")
            .changed()
        {
            commands.push(Command::TogglePreviewResizable);
        }
    }
}

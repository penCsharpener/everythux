//! Secondary windows: reindex confirmation, volumes, properties, shortcuts, about.

use std::fmt::Write as _;

use egui::{Context, Grid, RichText, Ui};
use everythux_index::ScanMode;

use crate::app::{App, Command, Effect};
use crate::format::{format_count, format_duration, format_local, format_permissions, format_size};
use crate::keys::describe_shortcut as d;
use crate::model::PropertiesInfo;
use crate::volumes::{DevicesState, RowState, VolumeRow, merge_rows};

enum VolumeAction {
    Enable(String, bool),
    Rescan(String, ScanMode),
    Mount(String),
    Refresh,
}

fn kind_text(kind: everythux_core::model::EntryKind) -> &'static str {
    use everythux_core::model::EntryKind;
    match kind {
        EntryKind::File => "File",
        EntryKind::Dir => "Folder",
        EntryKind::Symlink => "Symbolic link",
        EntryKind::Other => "Special file",
    }
}

fn selectable_value(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(text).selectable(true).wrap());
}

impl App {
    /// Shows every open secondary window.
    pub(crate) fn show_windows(&mut self, ctx: &Context) {
        self.confirm_reindex_modal(ctx);
        self.exclusion_popup_window(ctx);
        self.exclusion_editor_window(ctx);
        self.volumes_window(ctx);
        self.properties_windows(ctx);
        self.shortcuts_window(ctx);
        self.about_window(ctx);
        self.preview_width_window(ctx);
    }

    fn confirm_reindex_modal(&mut self, ctx: &Context) {
        if !self.windows.confirm_reindex {
            return;
        }
        let mut command = None;
        egui::Modal::new(egui::Id::new("everythux_confirm_reindex")).show(ctx, |ui| {
            ui.heading("Reindex in progress");
            ui.label("A reindex is in progress. Restart it?");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Restart").clicked() {
                    command = Some(Command::ConfirmFullReindex);
                }
                if ui.button("Keep running").clicked() {
                    command = Some(Command::Escape);
                }
            });
        });
        if let Some(command) = command {
            self.run_command(command);
        }
    }

    fn volumes_window(&mut self, ctx: &Context) {
        if !self.windows.volumes.open {
            return;
        }
        let mut open = true;
        let mut actions: Vec<VolumeAction> = Vec::new();
        let devices: &[everythux_platform::udisks::BlockDevice] = match &self.windows.volumes.devices {
            DevicesState::Loaded(d) => d,
            _ => &[],
        };
        let rows = merge_rows(&self.volumes, devices, &self.config.volumes.disabled_uuids);
        let loading = matches!(self.windows.volumes.devices, DevicesState::Loading);
        let unavailable = match &self.windows.volumes.devices {
            DevicesState::Unavailable(reason) => Some(reason.clone()),
            _ => None,
        };
        let date_format = self.config.ui.date_format.clone();
        let message = self.windows.volumes.message.clone();
        let mounting = self.windows.volumes.mounting.clone();

        egui::Window::new("Volumes")
            .id(egui::Id::new("everythux_volumes"))
            .open(&mut open)
            .default_size([760.0, 280.0])
            .show(ctx, |ui| {
                if let Some(reason) = &unavailable {
                    ui.colored_label(ui.visuals().warn_fg_color, "UDisks2 not available");
                    ui.weak(format!(
                        "Only volumes that are already indexed are listed. ({reason})"
                    ));
                }
                if rows.is_empty() && !loading {
                    ui.label("No volumes known yet.");
                }
                egui::ScrollArea::both()
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        Grid::new("volumes_grid")
                            .striped(true)
                            .num_columns(9)
                            .show(ui, |ui| {
                                for h in [
                                    "On",
                                    "Mount point",
                                    "Device",
                                    "Label",
                                    "UUID",
                                    "State",
                                    "Last scan",
                                    "",
                                    "",
                                ] {
                                    ui.label(RichText::new(h).strong());
                                }
                                ui.end_row();
                                for row in &rows {
                                    volume_row(ui, row, &date_format, &mounting, &mut actions);
                                    ui.end_row();
                                }
                            });
                    });
                ui.horizontal(|ui| {
                    if ui.button("Refresh").clicked() {
                        actions.push(VolumeAction::Refresh);
                    }
                    if loading {
                        ui.spinner();
                    }
                    if let Some(message) = &message {
                        ui.weak(message);
                    }
                });
            });

        self.windows.volumes.open = open;
        for action in actions {
            match action {
                VolumeAction::Enable(uuid, on) => self.set_volume_enabled(&uuid, on),
                VolumeAction::Rescan(uuid, mode) => self.rescan_volume(&uuid, mode),
                VolumeAction::Mount(object_path) => self.mount_and_index(object_path),
                VolumeAction::Refresh => self.refresh_devices(),
            }
        }
    }

    fn properties_windows(&mut self, ctx: &Context) {
        let size_format = self.config.ui.size_format;
        let date_format = self.config.ui.date_format.clone();
        let mut effects: Vec<Effect> = Vec::new();
        for window in &mut self.windows.properties {
            let name = window.target.path.file_name().map_or_else(
                || window.target.path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            let mut open = window.open;
            let mut close = false;
            egui::Window::new(format!("Properties: {name}"))
                .id(egui::Id::new(("everythux_properties", window.id)))
                .open(&mut open)
                .default_width(420.0)
                .show(ctx, |ui| {
                    let target = &window.target;
                    Grid::new(("properties_grid", window.id))
                        .num_columns(2)
                        .show(ui, |ui| {
                            row(ui, "Path", |ui| {
                                selectable_value(ui, &target.path.to_string_lossy());
                            });
                            row(ui, "Kind", |ui| {
                                ui.label(kind_text(target.kind));
                            });
                            row(ui, "Size", |ui| {
                                ui.label(format!(
                                    "{} ({} bytes)",
                                    format_size(target.size, size_format),
                                    format_count(target.size)
                                ));
                            });
                            if let Some(info) = &window.info {
                                properties_rows(ui, info, target.kind.is_dir(), &date_format);
                            } else {
                                row(ui, "Details", |ui| {
                                    ui.spinner();
                                });
                            }
                        });
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("Open folder").clicked() {
                            effects.push(if target.kind.is_dir() {
                                Effect::OpenDefault(target.path.clone())
                            } else {
                                Effect::ShowItem(target.path.clone())
                            });
                        }
                        if ui.button("Re-index").clicked() {
                            effects.push(Effect::Reindex {
                                path: target.folder().to_path_buf(),
                                mode: ScanMode::Full,
                            });
                        }
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                });
            window.open = open && !close;
        }
        self.apply_effects(effects);
    }

    fn shortcuts_window(&mut self, ctx: &Context) {
        if !self.windows.shortcuts {
            return;
        }
        let b = &self.bindings;
        let exclude = self.config.keybindings.exclude_path_click.clone();
        let entries: Vec<(String, &str, &str)> = vec![
            (
                "Typing".to_owned(),
                "anywhere",
                "Search (the search box is focused)",
            ),
            (
                d(b.reindex_all),
                "global",
                "Full reindex of all volumes (background, cancellable)",
            ),
            (
                "Double-click".to_owned(),
                "Name cell",
                "Open the file with its default application",
            ),
            (
                "Double-click".to_owned(),
                "Path cell",
                "Open the containing folder with the file selected",
            ),
            (
                "Double-click".to_owned(),
                "Size / Modified cell",
                "Open folder properties",
            ),
            (
                exclude,
                "Path cell",
                "Add the folder to the exclusions and open the exclusion popup",
            ),
            ("Right-click".to_owned(), "row", "Context menu"),
            (d(b.toggle_preview), "global", "Toggle the preview pane"),
            (
                "Enter / Ctrl+Enter".to_owned(),
                "row",
                "Open / open containing folder",
            ),
            (
                "Ctrl+C / Ctrl+Shift+C".to_owned(),
                "row",
                "Copy full path / copy folder path",
            ),
            (d(b.quick_rescan), "global", "Quick rescan"),
            (d(b.focus_search), "global", "Focus the first search bar"),
            (d(b.add_search_bar), "global", "Add a search bar (AND)"),
            (d(b.close_search_bar), "search bar", "Close this extra search bar"),
            (
                format!("{0} / Shift+{0}", d(b.switch_focus)),
                "global",
                "Switch between the result list and the newest search bar",
            ),
            (d(b.toggle_match_path), "global", "Match against the full path"),
            (
                "Escape".to_owned(),
                "global",
                "Close the topmost popup, or clear the focused search bar",
            ),
            ("Ctrl+Q".to_owned(), "global", "Quit"),
        ];
        let mut open = true;
        egui::Window::new("Keyboard shortcuts")
            .id(egui::Id::new("everythux_shortcuts"))
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                Grid::new("shortcuts_grid")
                    .striped(true)
                    .num_columns(3)
                    .show(ui, |ui| {
                        for h in ["Input", "Context", "Action"] {
                            ui.label(RichText::new(h).strong());
                        }
                        ui.end_row();
                        for (input, context, action) in &entries {
                            ui.label(RichText::new(input).monospace());
                            ui.label(*context);
                            ui.label(*action);
                            ui.end_row();
                        }
                    });
                ui.weak("All bindings can be overridden in config.json under “keybindings”.");
            });
        self.windows.shortcuts = open;
    }

    fn about_window(&mut self, ctx: &Context) {
        if !self.windows.about {
            return;
        }
        let mut open = true;
        egui::Window::new("About Everythux")
            .id(egui::Id::new("everythux_about"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.heading("Everythux");
                ui.label(format!(
                    "Version {} ({})",
                    env!("CARGO_PKG_VERSION"),
                    env!("EVERYTHUX_BUILD_ID")
                ));
                ui.label("Instant file search for Linux.");
                ui.weak("MIT OR Apache-2.0");
            });
        self.windows.about = open;
    }
}

fn row(ui: &mut Ui, key: &str, value: impl FnOnce(&mut Ui)) {
    ui.label(RichText::new(key).strong());
    value(ui);
    ui.end_row();
}

fn properties_rows(ui: &mut Ui, info: &PropertiesInfo, is_dir: bool, date_format: &str) {
    if is_dir {
        row(ui, "Contains", |ui| {
            ui.label(format!(
                "{} files, {} folders",
                format_count(info.files),
                format_count(info.folders)
            ));
        });
    }
    match &info.meta {
        Ok(m) => {
            row(ui, "Owner", |ui| {
                ui.label(format!("uid {}, gid {}", m.uid, m.gid));
            });
            row(ui, "Permissions", |ui| {
                ui.label(format!(
                    "{} ({:04o})",
                    format_permissions(m.mode),
                    m.mode & 0o7777
                ));
            });
            row(ui, "Modified", |ui| {
                ui.label(format_local(m.modified_ns, date_format));
            });
            row(ui, "Accessed", |ui| {
                ui.label(format_local(m.accessed_ns, date_format));
            });
            row(ui, "Changed", |ui| {
                ui.label(format_local(m.changed_ns, date_format));
            });
            if let Some(created) = m.created_ns {
                row(ui, "Created", |ui| {
                    ui.label(format_local(created, date_format));
                });
            }
        }
        Err(e) => row(ui, "On disk", |ui| {
            ui.colored_label(ui.visuals().warn_fg_color, e);
        }),
    }
}

fn volume_row(
    ui: &mut Ui,
    row: &VolumeRow,
    date_format: &str,
    mounting: &std::collections::HashSet<String>,
    actions: &mut Vec<VolumeAction>,
) {
    let uuid_key = row.uuid.clone().unwrap_or_default();
    let mut enabled = row.enabled;
    if row.uuid.is_some()
        && ui
            .add_enabled(true, egui::Checkbox::without_text(&mut enabled))
            .on_hover_text("Include this volume in the index")
            .changed()
    {
        actions.push(VolumeAction::Enable(uuid_key.clone(), enabled));
    } else if row.uuid.is_none() {
        ui.label("");
    }
    ui.label(
        row.mount_point
            .as_ref()
            .map_or_else(String::new, |p| p.display().to_string()),
    );
    ui.label(row.device.display().to_string());
    ui.label(row.label.clone().unwrap_or_default());
    ui.label(RichText::new(&uuid_key).monospace().small());
    let state = RichText::new(row.state.label());
    ui.label(match row.state {
        RowState::Online => state,
        RowState::Error => state.color(ui.visuals().error_fg_color),
        _ => state.color(ui.visuals().weak_text_color()),
    });
    let last = row.last_scan_ns.map_or_else(
        || "never".to_owned(),
        |ns| {
            let mut text = format_local(ns, date_format);
            if let Some(ms) = row.last_scan_ms {
                let _ = write!(
                    text,
                    " ({})",
                    format_duration(std::time::Duration::from_millis(ms))
                );
            }
            text
        },
    );
    ui.label(if row.indexed { last } else { String::new() });
    ui.horizontal(|ui| {
        let can_scan = row.indexed && row.uuid.is_some() && row.state != RowState::Offline;
        if ui
            .add_enabled(can_scan, egui::Button::new("Quick rescan"))
            .clicked()
        {
            actions.push(VolumeAction::Rescan(uuid_key.clone(), ScanMode::Quick));
        }
        if ui
            .add_enabled(can_scan, egui::Button::new("Full rescan"))
            .clicked()
        {
            actions.push(VolumeAction::Rescan(uuid_key.clone(), ScanMode::Full));
        }
    });
    if let Some(object_path) = &row.mountable {
        let busy = mounting.contains(object_path);
        if ui
            .add_enabled(!busy, egui::Button::new("Mount and index"))
            .clicked()
        {
            actions.push(VolumeAction::Mount(object_path.clone()));
        }
    } else {
        ui.label("");
    }
}

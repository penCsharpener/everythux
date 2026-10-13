//! The exclusion popup and the exclusion editor window.

use std::fmt::Write as _;
use std::path::Path;

use egui::{ComboBox, Context, Grid, RichText, ScrollArea, TextEdit, Ui};
use everythux_core::exclusion::{ExclusionRule, RuleKind};

use crate::app::{App, PopupId};
use crate::backend::ExclusionPreview;
use crate::exclusions::{ExclusionChoice, explain, rule_error};
use crate::format::format_count;

/// Warn in the popup when a rule would remove more than this share of the index.
const WARN_PERCENT: u64 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PopupAction {
    Undo,
    Done,
    OpenEditor,
}

fn kind_label(kind: RuleKind) -> &'static str {
    match kind {
        RuleKind::Dir => "Folder name",
        RuleKind::File => "File name",
        RuleKind::Path => "Path",
        RuleKind::Any => "Any",
    }
}

/// The editable state of the popup while it is being drawn.
struct PopupDraft {
    choice: ExclusionChoice,
    scope: String,
    custom: String,
    remove_now: bool,
    name: Option<String>,
    folder: String,
    error: Option<String>,
    preview: Option<ExclusionPreview>,
    total: u64,
}

/// Draws the popup contents; returns the button that was clicked, if any.
fn popup_body(ui: &mut Ui, d: &mut PopupDraft) -> Option<PopupAction> {
    let mut action = None;
    ui.add(egui::Label::new(RichText::new(&d.folder).monospace()).wrap());
    ui.separator();
    ui.radio_value(&mut d.choice, ExclusionChoice::ThisFolder, "This folder only");
    if let Some(name) = d.name.clone() {
        ui.radio_value(
            &mut d.choice,
            ExclusionChoice::AnyNamed,
            format!("Any folder named “{name}”"),
        );
        ui.radio_value(
            &mut d.choice,
            ExclusionChoice::AnyNamedUnder,
            format!("Any folder named “{name}” under:"),
        );
        ui.horizontal(|ui| {
            ui.add_space(22.0);
            let r = ui.add(
                TextEdit::singleline(&mut d.scope)
                    .hint_text("/home/*")
                    .desired_width(200.0),
            );
            if r.changed() {
                d.choice = ExclusionChoice::AnyNamedUnder;
            }
        });
    }
    ui.radio_value(&mut d.choice, ExclusionChoice::Custom, "Custom glob:");
    ui.horizontal(|ui| {
        ui.add_space(22.0);
        let r = ui.add(
            TextEdit::singleline(&mut d.custom)
                .hint_text("**/node_modules or re:^\\.cache-\\d+$")
                .desired_width(260.0),
        );
        if r.changed() {
            d.choice = ExclusionChoice::Custom;
        }
    });
    if let Some(error) = &d.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    ui.separator();
    match &d.preview {
        Some(p) if p.error.is_none() => {
            ui.label(format!(
                "Matches {} indexed entries in {} folders",
                format_count(p.entries),
                format_count(p.folders)
            ));
            if d.total > 0 && p.entries * 100 / d.total >= WARN_PERCENT {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("This removes {}% of the index.", p.entries * 100 / d.total),
                );
            }
        }
        Some(p) => {
            ui.colored_label(ui.visuals().error_fg_color, p.error.clone().unwrap_or_default());
        }
        None if d.error.is_none() => {
            ui.weak("Counting matches…");
        }
        None => {}
    }
    ui.add_space(4.0);
    ui.radio_value(
        &mut d.remove_now,
        true,
        "Remove matching entries from the index now",
    );
    ui.radio_value(&mut d.remove_now, false, "Keep until next rescan");
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Undo").clicked() {
            action = Some(PopupAction::Undo);
        }
        if ui.button("Open exclusion editor").clicked() {
            action = Some(PopupAction::OpenEditor);
        }
        if ui
            .add_enabled(d.error.is_none(), egui::Button::new("Done"))
            .clicked()
        {
            action = Some(PopupAction::Done);
        }
    });
    action
}

impl App {
    pub(crate) fn exclusion_popup_window(&mut self, ctx: &Context) {
        let Some(popup) = &self.windows.exclusion_popup else {
            return;
        };
        let mut draft = PopupDraft {
            choice: popup.choice,
            scope: popup.scope.clone(),
            custom: popup.custom.clone(),
            remove_now: popup.remove_now,
            name: popup.name.clone(),
            folder: popup.folder.to_string_lossy().into_owned(),
            error: popup.error.clone(),
            preview: popup.preview.clone(),
            total: popup.snapshot.len() as u64,
        };
        let anchor = popup.anchor;

        let mut window = egui::Window::new("Exclude folder")
            .id(egui::Id::new("everythux_exclusion_popup"))
            .collapsible(false)
            .resizable(false)
            .default_width(380.0);
        if let Some(pos) = anchor {
            window = window.default_pos(pos + egui::vec2(10.0, 10.0));
        }
        let action = window
            .show(ctx, |ui| popup_body(ui, &mut draft))
            .and_then(|r| r.inner)
            .flatten();
        let PopupDraft {
            choice,
            scope,
            custom,
            remove_now,
            ..
        } = draft;

        // Write the edits back, then act on what was clicked.
        let Some(popup) = &mut self.windows.exclusion_popup else {
            return;
        };
        popup.remove_now = remove_now;
        let scope_changed = popup.scope != scope;
        let custom_changed = popup.custom != custom;
        popup.scope = scope;
        popup.custom = custom;
        if choice != popup.choice {
            self.set_exclusion_choice(choice);
        } else if (scope_changed && choice == ExclusionChoice::AnyNamedUnder)
            || (custom_changed && choice == ExclusionChoice::Custom)
        {
            self.exclusion_popup_text_edited();
        }
        match action {
            Some(PopupAction::Undo) => self.undo_exclusion(),
            Some(PopupAction::Done) => self.close_popup(PopupId::ExclusionPopup),
            Some(PopupAction::OpenEditor) => {
                self.finish_exclusion_popup();
                self.open_exclusion_editor();
            }
            None => {}
        }
    }

    pub(crate) fn exclusion_editor_window(&mut self, ctx: &Context) {
        if !self.windows.exclusion_editor.open {
            return;
        }
        let mut open = true;
        let mut rules = self.config.exclusions.clone();
        let mut test_path = self.windows.exclusion_editor.test_path.clone();
        let mut test_is_dir = self.windows.exclusion_editor.test_is_dir;
        let mut transfer_path = self.windows.exclusion_editor.transfer_path.clone();
        let mut confirm_restore = self.windows.exclusion_editor.confirm_restore;
        let message = self.windows.exclusion_editor.message.clone();
        let pending = !self.pending_reindex_jobs().is_empty();
        let (mut do_import, mut do_export, mut do_restore, mut do_reindex, mut do_dismiss) =
            (false, false, false, false, false);

        egui::Window::new("Exclusions")
            .id(egui::Id::new("everythux_exclusion_editor"))
            .open(&mut open)
            .default_size([720.0, 460.0])
            .show(ctx, |ui| {
                ui.weak("Folders and files matching an enabled rule are not indexed. Rules apply in order.");
                rules_table(ui, &mut rules);
                ui.separator();

                test_path_section(ui, &rules, &mut test_path, &mut test_is_dir);
                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("File:");
                    ui.add(
                        TextEdit::singleline(&mut transfer_path)
                            .hint_text("/path/to/rules.json")
                            .desired_width(320.0),
                    );
                    do_import = ui.button("Import").clicked();
                    do_export = ui.button("Export").clicked();
                });
                ui.horizontal(|ui| {
                    if confirm_restore {
                        ui.label("Replace all rules with the defaults?");
                        do_restore = ui.button("Yes, restore").clicked();
                        if ui.button("Cancel").clicked() {
                            confirm_restore = false;
                        }
                    } else if ui.button("Restore defaults").clicked() {
                        confirm_restore = true;
                    }
                });
                if pending {
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Rules were removed or changed. Entries they hid return after a re-index.");
                        do_reindex = ui.button("Re-index affected folders").clicked();
                        do_dismiss = ui.button("Dismiss").clicked();
                    });
                }
                if let Some(message) = &message {
                    ui.weak(message);
                }
            });

        let editor = &mut self.windows.exclusion_editor;
        editor.open = open;
        editor.test_path = test_path;
        editor.test_is_dir = test_is_dir;
        editor.transfer_path = transfer_path;
        editor.confirm_restore = confirm_restore;
        self.set_exclusion_rules(rules);
        if do_restore {
            self.restore_default_exclusions();
        }
        if do_import {
            self.import_exclusions();
        }
        if do_export {
            self.export_exclusions();
        }
        if do_reindex {
            self.run_pending_reindex();
        }
        if do_dismiss {
            self.dismiss_pending_reindex();
        }
    }
}

/// "Test a path": which rule (if any) would exclude the typed path.
fn test_path_section(ui: &mut Ui, rules: &[ExclusionRule], test_path: &mut String, is_dir: &mut bool) {
    ui.horizontal(|ui| {
        ui.label("Test a path:");
        ui.add(
            TextEdit::singleline(test_path)
                .hint_text("/home/me/proj/bin/x")
                .desired_width(320.0),
        );
        ui.checkbox(is_dir, "is a folder");
    });
    if test_path.trim().is_empty() {
        return;
    }
    let path = Path::new(test_path.trim());
    match explain(rules, path, *is_dir) {
        Some(e) => {
            let mut text = format!(
                "Excluded by rule #{}: {}",
                e.rule_index + 1,
                rules[e.rule_index].pattern
            );
            if e.matched != path {
                let _ = write!(text, " (matches {})", e.matched.display());
            }
            ui.colored_label(ui.visuals().warn_fg_color, text);
        }
        None => {
            ui.label("Not excluded");
        }
    }
}

/// The editable rule grid. Edits are applied to `rules`; the caller persists them.
fn rules_table(ui: &mut Ui, rules: &mut Vec<ExclusionRule>) {
    let errors: Vec<Option<String>> = rules.iter().map(rule_error).collect();
    let mut remove = None;
    let mut swap = None;
    let mut add = false;
    ScrollArea::vertical()
        .max_height(260.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            Grid::new("exclusion_rules")
                .striped(true)
                .num_columns(6)
                .show(ui, |ui| {
                    for h in ["On", "Pattern", "Applies to", "Scope", "", ""] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    let last = rules.len().saturating_sub(1);
                    for (i, rule) in rules.iter_mut().enumerate() {
                        ui.checkbox(&mut rule.enabled, "").on_hover_text("Enabled");
                        ui.add(TextEdit::singleline(&mut rule.pattern).desired_width(210.0));
                        ComboBox::from_id_salt(("rule_kind", i))
                            .selected_text(kind_label(rule.kind))
                            .width(100.0)
                            .show_ui(ui, |ui| {
                                for kind in [RuleKind::Dir, RuleKind::File, RuleKind::Path, RuleKind::Any] {
                                    ui.selectable_value(&mut rule.kind, kind, kind_label(kind));
                                }
                            });
                        let mut scope = rule.scope.clone().unwrap_or_default();
                        if ui
                            .add(
                                TextEdit::singleline(&mut scope)
                                    .hint_text("(everywhere)")
                                    .desired_width(130.0),
                            )
                            .changed()
                        {
                            rule.scope = (!scope.trim().is_empty()).then_some(scope);
                        }
                        ui.horizontal(|ui| {
                            if ui.add_enabled(i > 0, egui::Button::new("Up")).clicked() {
                                swap = Some((i, i - 1));
                            }
                            if ui.add_enabled(i < last, egui::Button::new("Down")).clicked() {
                                swap = Some((i, i + 1));
                            }
                        });
                        if ui.button("Delete").clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                        if let Some(error) = &errors[i] {
                            ui.label("");
                            ui.colored_label(ui.visuals().error_fg_color, error);
                            ui.end_row();
                        }
                    }
                });
            if ui.button("Add rule").clicked() {
                add = true;
            }
        });
    if let Some(i) = remove {
        rules.remove(i);
    } else if let Some((a, b)) = swap {
        rules.swap(a, b);
    }
    if add {
        rules.push(ExclusionRule::new("", RuleKind::Dir));
    }
}

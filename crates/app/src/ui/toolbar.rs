//! The icon toolbar under the menu bar: search switches, indexing actions, config and preview.

use egui::Ui;

use crate::app::{App, Command};
use crate::keys::describe_shortcut;
use crate::ui::icons::{Glyph, icon_button};

/// Hover text, with the shortcut in parentheses when there is one.
fn tip(text: &str, shortcut: &str) -> String {
    if shortcut.is_empty() {
        text.to_owned()
    } else {
        format!("{text} ({shortcut})")
    }
}

impl App {
    pub(crate) fn toolbar(&mut self, ui: &mut Ui) {
        let s = &self.config.search;
        let b = &self.bindings;
        let buttons = [
            (
                Glyph::MatchCase,
                tip("Match case", ""),
                Some(s.case_sensitive),
                Command::ToggleCaseSensitive,
            ),
            (
                Glyph::MatchPath,
                tip("Match path", &describe_shortcut(b.toggle_match_path)),
                Some(s.match_path),
                Command::ToggleMatchPath,
            ),
            (
                Glyph::Regex,
                tip("Regex", ""),
                Some(s.regex),
                Command::ToggleRegex,
            ),
            (
                Glyph::Duplicates,
                tip("Show only duplicates (same name and size)", ""),
                Some(self.duplicates_only),
                Command::ToggleDuplicates,
            ),
            (
                Glyph::Reindex,
                tip("Reindex all", &describe_shortcut(b.reindex_all)),
                None,
                Command::FullReindex,
            ),
            (
                Glyph::QuickRescan,
                tip("Quick rescan", &describe_shortcut(b.quick_rescan)),
                None,
                Command::QuickRescan,
            ),
            (
                Glyph::Config,
                tip("Open JSON config", ""),
                None,
                Command::OpenConfig,
            ),
            (
                Glyph::Star,
                tip("Add the current search to the bookmarks…", ""),
                None,
                Command::AddBookmark,
            ),
            (
                Glyph::Preview,
                tip("Show preview", &describe_shortcut(b.toggle_preview)),
                Some(self.config.ui.preview_pane.enabled),
                Command::TogglePreview,
            ),
        ];
        let mut commands = Vec::new();
        ui.horizontal(|ui| {
            for (i, (glyph, label, state, command)) in buttons.into_iter().enumerate() {
                // Separators: switches | index actions | config and preview.
                if i == 4 || i == 6 {
                    ui.separator();
                }
                let response = icon_button(ui, glyph, &label, true, state.unwrap_or(false));
                if response.on_hover_text(&label).clicked() {
                    commands.push(command);
                }
            }
        });
        for command in commands {
            self.run_command(command);
        }
    }
}

//! Config-backed settings: every change is persisted (debounced by the backend) and, where it
//! matters, pushed to the indexer or turned into a new search.

use everythux_core::config::{ColumnId, SizeFormat, Theme};
use everythux_core::exclusion::ExclusionRule;
use everythux_index::IndexerSettings;

use super::App;

/// Rules that compile. Invalid ones stay in the config so the user can fix them, but must not be
/// handed to the indexer.
pub(crate) fn valid_rules(rules: &[ExclusionRule]) -> Vec<ExclusionRule> {
    rules
        .iter()
        .filter(|r| crate::exclusions::rule_error(r).is_none())
        .cloned()
        .collect()
}

impl App {
    /// Persists the config.
    pub(crate) fn config_changed(&mut self) {
        self.backend.save_config(&self.config);
    }

    /// The indexer's view of the config.
    pub(crate) fn indexer_settings(&self) -> IndexerSettings {
        IndexerSettings {
            volumes: self.config.volumes.clone(),
            exclusions: valid_rules(&self.config.exclusions),
            watch_ignore: Vec::new(),
            indexer: self.config.indexer.clone(),
        }
    }

    /// Persists the config and pushes volume / exclusion settings to the indexer.
    pub(crate) fn settings_changed(&mut self) {
        self.backend.update_settings(self.indexer_settings());
        self.config_changed();
    }

    /// Applies `config.ui.theme` to egui.
    pub(crate) fn apply_theme(&self) {
        let preference = match self.config.ui.theme {
            Theme::System => egui::ThemePreference::System,
            Theme::Light => egui::ThemePreference::Light,
            Theme::Dark => egui::ThemePreference::Dark,
        };
        self.ctx.set_theme(preference);
    }

    pub(crate) fn set_theme(&mut self, theme: Theme) {
        if self.config.ui.theme != theme {
            self.config.ui.theme = theme;
            self.apply_theme();
            self.config_changed();
        }
    }

    pub(crate) fn set_size_format(&mut self, format: SizeFormat) {
        if self.config.ui.size_format != format {
            self.config.ui.size_format = format;
            self.config_changed();
        }
    }

    /// Shows or hides a column. The last visible column cannot be hidden.
    pub(crate) fn toggle_column(&mut self, id: ColumnId) {
        let visible_count = self.config.ui.columns.iter().filter(|c| c.visible).count();
        if let Some(col) = self.config.ui.columns.iter_mut().find(|c| c.id == id) {
            if col.visible && visible_count <= 1 {
                return;
            }
            col.visible = !col.visible;
            self.config_changed();
        }
    }

    pub(crate) fn toggle_case_sensitive(&mut self) {
        self.config.search.case_sensitive = !self.config.search.case_sensitive;
        self.config_changed();
        self.resubmit_search();
    }

    pub(crate) fn toggle_match_path(&mut self) {
        self.config.search.match_path = !self.config.search.match_path;
        self.config_changed();
        self.resubmit_search();
    }

    pub(crate) fn toggle_regex(&mut self) {
        self.config.search.regex = !self.config.search.regex;
        self.config_changed();
        self.resubmit_search();
    }

    pub(crate) fn toggle_preview_pane(&mut self) {
        self.config.ui.preview_pane.enabled = !self.config.ui.preview_pane.enabled;
        if !self.config.ui.preview_pane.enabled {
            self.preview.clear();
        }
        self.config_changed();
    }

    /// Sets the fixed preview pane width (menu presets and the Custom… window) and persists it.
    pub(crate) fn set_preview_width(&mut self, width: f32) {
        let pane = &mut self.config.ui.preview_pane;
        if width.is_finite() && (pane.width - width).abs() > 0.5 {
            pane.width = width;
            self.config_changed();
        }
    }

    pub(crate) fn toggle_preview_resizable(&mut self) {
        let pane = &mut self.config.ui.preview_pane;
        pane.resizable = !pane.resizable;
        self.config_changed();
    }

    pub(crate) fn toggle_lock_column_widths(&mut self) {
        let ui = &mut self.config.ui;
        ui.lock_column_widths = !ui.lock_column_widths;
        self.config_changed();
    }

    /// Header click: sorts by `column`, flipping the direction if it is already the sort column.
    pub(crate) fn sort_by(&mut self, column: ColumnId) {
        let sort = &mut self.config.ui.sort;
        if sort.column == column {
            sort.ascending = !sort.ascending;
        } else {
            sort.column = column;
            sort.ascending = true;
        }
        self.config_changed();
        self.resubmit_search();
    }

    /// Records the table's current column widths. Persists only when the pointer is up, so a drag
    /// doesn't rewrite the file every frame.
    pub(crate) fn update_column_widths(&mut self, widths: &[(ColumnId, f32)], dragging: bool) {
        let mut changed = false;
        for &(id, width) in widths {
            if let Some(col) = self.config.ui.columns.iter_mut().find(|c| c.id == id)
                && (col.width - width).abs() > 0.5
            {
                col.width = width;
                changed = true;
            }
        }
        if changed && !dragging {
            self.config_changed();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::tests::TestApp;

    use super::*;

    #[test]
    fn invalid_rules_are_not_sent_to_the_indexer() {
        use everythux_core::exclusion::RuleKind;
        let good = ExclusionRule::new("node_modules", RuleKind::Dir);
        let bad = ExclusionRule::new("[", RuleKind::Any);
        assert_eq!(valid_rules(&[good.clone(), bad]), vec![good]);
    }

    #[test]
    fn toggling_search_options_persists_and_resubmits() {
        let mut t = TestApp::new();
        let before = t.fake.searches().len();
        t.app.toggle_match_path();
        t.app.toggle_regex();
        t.app.toggle_case_sensitive();
        let searches = t.fake.searches();
        assert_eq!(searches.len(), before + 3);
        let last = searches.last().expect("search");
        assert!(last.options.match_path && last.options.regex && last.options.case_sensitive);
        let saved = t.fake.last_saved_config().expect("saved");
        assert!(saved.search.match_path && saved.search.regex && saved.search.case_sensitive);
    }

    #[test]
    fn locking_column_widths_persists() {
        let mut t = TestApp::new();
        assert!(!t.app.config.ui.lock_column_widths);
        t.app.toggle_lock_column_widths();
        assert!(t.fake.last_saved_config().expect("saved").ui.lock_column_widths);
        t.app.toggle_lock_column_widths();
        assert!(!t.fake.last_saved_config().expect("saved").ui.lock_column_widths);
    }

    #[test]
    fn last_visible_column_cannot_be_hidden() {
        let mut t = TestApp::new();
        for id in [ColumnId::Path, ColumnId::Type, ColumnId::Modified, ColumnId::Size] {
            t.app.toggle_column(id);
        }
        t.app.toggle_column(ColumnId::Name);
        assert!(
            t.app
                .config
                .ui
                .columns
                .iter()
                .find(|c| c.id == ColumnId::Name)
                .expect("name")
                .visible
        );
        t.app.toggle_column(ColumnId::Size);
        assert!(
            t.app
                .config
                .ui
                .columns
                .iter()
                .find(|c| c.id == ColumnId::Size)
                .expect("size")
                .visible
        );
    }

    #[test]
    fn column_widths_persist_only_when_not_dragging() {
        let mut t = TestApp::new();
        t.fake.clear();
        t.app.update_column_widths(&[(ColumnId::Name, 333.0)], true);
        assert!(t.fake.last_saved_config().is_none(), "no save mid-drag");
        t.app.update_column_widths(&[(ColumnId::Name, 340.0)], false);
        let saved = t.fake.last_saved_config().expect("saved after drag");
        let width = saved
            .ui
            .columns
            .iter()
            .find(|c| c.id == ColumnId::Name)
            .expect("name")
            .width;
        assert!((width - 340.0).abs() < f32::EPSILON);
    }
}

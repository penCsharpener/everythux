//! App behaviour of the exclusion popup and editor.

use std::path::PathBuf;

use everythux_core::exclusion::{ExclusionRule, default_rules};
use everythux_index::Job;

use super::{ExclusionChoice, ExclusionPopup, reindex_jobs, removed_rules};
use crate::app::{App, PopupId};
use crate::backend::{Compute, ExclusionPreview};
use crate::status::{ToastKind, ToastUndo};

impl App {
    /// Hands out ids for asynchronous requests (previews, properties windows).
    pub(crate) fn next_request_id(&mut self) -> u64 {
        self.windows.next_id += 1;
        self.windows.next_id
    }

    /// `Ctrl+Shift+Click` on a Path cell: adds a `Path` rule for `folder` right away, tells the
    /// indexer, and opens the popup that lets the user refine or undo it.
    pub(crate) fn begin_exclusion(&mut self, folder: PathBuf) {
        if let Some(existing) = &self.windows.exclusion_popup {
            if existing.folder == folder {
                self.open_popup(PopupId::ExclusionPopup);
                return;
            }
            self.finish_exclusion_popup();
        }
        let snapshot = self
            .results
            .as_ref()
            .map_or_else(|| self.snapshot.clone(), |r| r.snapshot.clone());
        let popup = ExclusionPopup::new(folder, snapshot, self.pointer);
        if !self.config.exclusions.contains(&popup.current_rule) {
            self.config.exclusions.push(popup.current_rule.clone());
        }
        self.settings_changed();
        self.windows.exclusion_popup = Some(popup);
        self.open_popup(PopupId::ExclusionPopup);
        self.request_exclusion_preview();
    }

    /// Switches between the popup's rule kinds, replacing the rule that was just added.
    pub(crate) fn set_exclusion_choice(&mut self, choice: ExclusionChoice) {
        if let Some(popup) = &mut self.windows.exclusion_popup {
            popup.choice = choice;
        }
        self.replace_popup_rule();
    }

    /// Called after the scope or custom-glob text changed.
    pub(crate) fn exclusion_popup_text_edited(&mut self) {
        self.replace_popup_rule();
    }

    fn replace_popup_rule(&mut self) {
        let Some(popup) = &mut self.windows.exclusion_popup else {
            return;
        };
        let candidate = popup.candidate();
        if let Some(e) = crate::exclusions::rule_error(&candidate) {
            popup.error = Some(e);
            popup.preview = None;
            return;
        }
        popup.error = None;
        if candidate != popup.current_rule {
            let old = std::mem::replace(&mut popup.current_rule, candidate.clone());
            match self.config.exclusions.iter().position(|r| *r == old) {
                Some(i) => self.config.exclusions[i] = candidate,
                None => self.config.exclusions.push(candidate),
            }
            self.settings_changed();
        }
        self.request_exclusion_preview();
    }

    fn request_exclusion_preview(&mut self) {
        let request = self.next_request_id();
        let Some(popup) = &mut self.windows.exclusion_popup else {
            return;
        };
        popup.preview_request = request;
        let task = Compute::ExclusionPreview {
            request,
            snapshot: popup.snapshot.clone(),
            rules: vec![popup.current_rule.clone()],
        };
        self.backend.compute(task);
    }

    pub(crate) fn on_exclusion_preview(&mut self, preview: ExclusionPreview) {
        if let Some(popup) = &mut self.windows.exclusion_popup
            && popup.preview_request == preview.request
        {
            popup.preview = Some(preview);
        }
    }

    /// "Undo": removes the rule the popup added and closes it.
    pub(crate) fn undo_exclusion(&mut self) {
        let Some(popup) = self.windows.exclusion_popup.take() else {
            return;
        };
        self.windows.order.retain(|p| *p != PopupId::ExclusionPopup);
        self.remove_rule(&popup.current_rule);
        self.toast(
            format!("Removed exclusion {}", popup.current_rule.pattern),
            ToastKind::Info,
        );
    }

    /// "Done" (and Escape): keeps the rule and, if asked, removes matching entries from the index.
    pub(crate) fn finish_exclusion_popup(&mut self) {
        let Some(popup) = self.windows.exclusion_popup.take() else {
            return;
        };
        self.windows.order.retain(|p| *p != PopupId::ExclusionPopup);
        let rule = popup.current_rule;
        if !self.config.exclusions.contains(&rule) {
            return;
        }
        let applied = popup.remove_now;
        if applied {
            self.backend.submit_job(Job::ApplyExclusions);
        }
        self.toast_with_undo(
            format!("Added exclusion {}", rule.pattern),
            ToastUndo::Exclusion { rule, applied },
        );
    }

    /// Removes the first config rule equal to `rule` and informs the indexer.
    fn remove_rule(&mut self, rule: &ExclusionRule) -> bool {
        let Some(i) = self.config.exclusions.iter().position(|r| r == rule) else {
            return false;
        };
        self.config.exclusions.remove(i);
        self.settings_changed();
        true
    }

    /// The "Undo" button of a status-bar message.
    pub(crate) fn undo_toast(&mut self, undo: ToastUndo) {
        self.toasts.clear();
        match undo {
            ToastUndo::Exclusion { rule, applied } => {
                if self.remove_rule(&rule) {
                    if applied {
                        for job in reindex_jobs(std::slice::from_ref(&rule)) {
                            self.backend.submit_job(job);
                        }
                    }
                    self.toast(format!("Removed exclusion {}", rule.pattern), ToastKind::Info);
                }
            }
            ToastUndo::Rename { current, restore } => self.undo_rename(current, restore),
        }
    }

    // ---- editor ----

    /// Index → Exclusions…
    pub(crate) fn open_exclusion_editor(&mut self) {
        let editor = &mut self.windows.exclusion_editor;
        if !editor.open {
            editor.baseline.clone_from(&self.config.exclusions);
            editor.message = None;
            editor.confirm_restore = false;
        }
        editor.open = true;
        self.open_popup(PopupId::ExclusionEditor);
    }

    /// Takes over rules edited in the editor window.
    pub(crate) fn set_exclusion_rules(&mut self, rules: Vec<ExclusionRule>) {
        if rules != self.config.exclusions {
            self.config.exclusions = rules;
            self.settings_changed();
        }
    }

    /// Jobs that would restore entries hidden by rules the user removed or disabled.
    pub(crate) fn pending_reindex_jobs(&self) -> Vec<Job> {
        let removed = removed_rules(&self.windows.exclusion_editor.baseline, &self.config.exclusions);
        reindex_jobs(&removed)
    }

    /// "Re-index affected folders".
    pub(crate) fn run_pending_reindex(&mut self) {
        let jobs = self.pending_reindex_jobs();
        let count = jobs.len();
        for job in jobs {
            self.backend.submit_job(job);
        }
        self.windows.exclusion_editor.baseline = self.config.exclusions.clone();
        self.toast(format!("Re-indexing {count} location(s)"), ToastKind::Info);
    }

    /// Hides the re-index offer without acting on it.
    pub(crate) fn dismiss_pending_reindex(&mut self) {
        self.windows.exclusion_editor.baseline = self.config.exclusions.clone();
    }

    pub(crate) fn restore_default_exclusions(&mut self) {
        self.windows.exclusion_editor.confirm_restore = false;
        self.set_exclusion_rules(default_rules());
        self.windows.exclusion_editor.message = Some("Restored the default exclusions".to_owned());
    }

    pub(crate) fn export_exclusions(&mut self) {
        let path = self.windows.exclusion_editor.transfer_path.trim().to_owned();
        if path.is_empty() {
            self.windows.exclusion_editor.message = Some("Enter a file path first".to_owned());
            return;
        }
        let rules = self.config.exclusions.clone();
        self.backend.compute(Compute::ExportRules {
            path: PathBuf::from(path),
            rules,
        });
    }

    pub(crate) fn import_exclusions(&mut self) {
        let path = self.windows.exclusion_editor.transfer_path.trim().to_owned();
        if path.is_empty() {
            self.windows.exclusion_editor.message = Some("Enter a file path first".to_owned());
            return;
        }
        self.backend.compute(Compute::ImportRules(PathBuf::from(path)));
    }

    pub(crate) fn on_rules_imported(&mut self, result: Result<Vec<ExclusionRule>, String>) {
        let message = match result {
            Ok(rules) => {
                let total = rules.len();
                let mut merged = self.config.exclusions.clone();
                for rule in rules {
                    if !merged.contains(&rule) {
                        merged.push(rule);
                    }
                }
                let added = merged.len() - self.config.exclusions.len();
                self.set_exclusion_rules(merged);
                format!("Imported {added} new rule(s) from {total}")
            }
            Err(e) => format!("Import failed: {e}"),
        };
        self.windows.exclusion_editor.message = Some(message);
    }

    pub(crate) fn on_rules_exported(&mut self, result: Result<(PathBuf, usize), String>) {
        self.windows.exclusion_editor.message = Some(match result {
            Ok((path, n)) => format!("Exported {n} rule(s) to {}", path.display()),
            Err(e) => format!("Export failed: {e}"),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use egui::Modifiers;
    use everythux_core::config::ColumnId;
    use everythux_core::exclusion::RuleKind;
    use everythux_index::ScanMode;

    use super::*;
    use crate::app::tests::TestApp;
    use crate::app::{CellEvent, Command};
    use crate::backend::BackendEvent;
    use crate::backend::fake::Call;

    fn open_popup(t: &mut TestApp) {
        t.show_results(&[t.fixture.junk]);
        let mods = Modifiers::CTRL | Modifiers::SHIFT;
        let fx = t.app.on_cell_event(0, ColumnId::Path, CellEvent::Click(mods));
        t.app.apply_effects(fx);
        t.pump();
    }

    fn rules(t: &TestApp) -> &[ExclusionRule] {
        &t.app.config.exclusions
    }

    #[test]
    fn popup_prefills_a_path_rule_and_computes_a_preview() {
        let mut t = TestApp::new();
        open_popup(&mut t);
        let popup = t.app.windows.exclusion_popup.as_ref().expect("popup");
        assert_eq!(popup.folder, Path::new("/home/me/proj/node_modules"));
        assert_eq!(popup.name.as_deref(), Some("node_modules"));
        assert!(popup.remove_now, "default is to remove now");
        assert_eq!(popup.scope, "/home/*");
        let preview = popup.preview.as_ref().expect("preview arrived");
        assert_eq!(preview.entries, 3, "the folder itself plus junk.js and more.js");
        assert_eq!(preview.folders, 1);
        assert!(rules(&t).contains(&ExclusionRule::new("/home/me/proj/node_modules", RuleKind::Path)));
        assert_eq!(t.app.windows.order.last(), Some(&PopupId::ExclusionPopup));
    }

    #[test]
    fn switching_options_replaces_the_rule() {
        let mut t = TestApp::new();
        let initial = {
            open_popup(&mut t);
            rules(&t).len()
        };
        t.app.set_exclusion_choice(ExclusionChoice::AnyNamed);
        assert_eq!(rules(&t).len(), initial, "replaced, not appended");
        assert!(rules(&t).contains(&ExclusionRule::new("node_modules", RuleKind::Dir)));
        assert!(!rules(&t).contains(&ExclusionRule::new("/home/me/proj/node_modules", RuleKind::Path)));

        t.app.set_exclusion_choice(ExclusionChoice::AnyNamedUnder);
        assert!(rules(&t).contains(&ExclusionRule::new("node_modules", RuleKind::Dir).scoped("/home/*")));
        assert_eq!(rules(&t).len(), initial);
    }

    #[test]
    fn custom_glob_is_validated_live() {
        let mut t = TestApp::new();
        open_popup(&mut t);
        let before: Vec<_> = rules(&t).to_vec();
        t.app.set_exclusion_choice(ExclusionChoice::Custom);
        let popup = t.app.windows.exclusion_popup.as_mut().expect("popup");
        popup.custom = "[".to_owned();
        t.app.exclusion_popup_text_edited();
        let popup = t.app.windows.exclusion_popup.as_ref().expect("popup");
        assert!(popup.error.is_some(), "invalid glob reported");
        assert!(popup.preview.is_none());
        assert!(
            !rules(&t).iter().any(|r| r.pattern == "["),
            "invalid rules never reach the config"
        );
        assert_eq!(rules(&t).len(), before.len());

        let popup = t.app.windows.exclusion_popup.as_mut().expect("popup");
        popup.custom = "**/node_modules".to_owned();
        t.app.exclusion_popup_text_edited();
        t.pump();
        let popup = t.app.windows.exclusion_popup.as_ref().expect("popup");
        assert_eq!(popup.error, None);
        assert_eq!(popup.preview.as_ref().expect("preview").entries, 3);
    }

    #[test]
    fn undo_removes_the_rule_and_closes_the_popup() {
        let mut t = TestApp::new();
        let before = t.app.config.exclusions.clone();
        open_popup(&mut t);
        assert_ne!(rules(&t), before.as_slice());
        t.app.undo_exclusion();
        assert_eq!(rules(&t), before.as_slice());
        assert!(t.app.windows.exclusion_popup.is_none());
        let saved = t.fake.last_saved_config().expect("saved");
        assert_eq!(saved.exclusions, before);
        assert_eq!(t.fake.last_settings().expect("settings").exclusions, before);
        assert!(
            !t.fake.jobs().contains(&Job::ApplyExclusions),
            "nothing was applied"
        );
    }

    #[test]
    fn done_applies_exclusions_when_asked() {
        let mut t = TestApp::new();
        open_popup(&mut t);
        t.app.finish_exclusion_popup();
        assert_eq!(t.fake.jobs(), [Job::ApplyExclusions]);
        assert!(t.app.windows.exclusion_popup.is_none());
        let toast = t.app.toasts.last().expect("toast");
        assert!(toast.text.starts_with("Added exclusion"));
        assert!(toast.undo.is_some());
    }

    #[test]
    fn keep_until_rescan_does_not_touch_the_index() {
        let mut t = TestApp::new();
        open_popup(&mut t);
        t.app.windows.exclusion_popup.as_mut().expect("popup").remove_now = false;
        t.app.finish_exclusion_popup();
        assert!(t.fake.jobs().is_empty(), "{:?}", t.fake.jobs());
        assert!(rules(&t).contains(&ExclusionRule::new("/home/me/proj/node_modules", RuleKind::Path)));
    }

    #[test]
    fn escape_commits_the_popup() {
        let mut t = TestApp::new();
        open_popup(&mut t);
        t.app.run_command(Command::Escape);
        assert!(t.app.windows.exclusion_popup.is_none());
        assert_eq!(t.fake.jobs(), [Job::ApplyExclusions]);
    }

    #[test]
    fn undo_after_apply_reindexes_the_folder() {
        let mut t = TestApp::new();
        open_popup(&mut t);
        t.app.finish_exclusion_popup();
        t.fake.clear();
        let undo = t
            .app
            .toasts
            .last()
            .and_then(|toast| toast.undo.clone())
            .expect("undo");
        t.app.undo_toast(undo);
        assert!(!rules(&t).contains(&ExclusionRule::new("/home/me/proj/node_modules", RuleKind::Path)));
        assert_eq!(
            t.fake.jobs(),
            [Job::RescanPath {
                path: "/home/me/proj/node_modules".into(),
                mode: ScanMode::Full
            }]
        );
    }

    #[test]
    fn a_second_exclusion_commits_the_first_popup() {
        let mut t = TestApp::new();
        open_popup(&mut t);
        t.app.begin_exclusion("/home/me/docs".into());
        let popup = t.app.windows.exclusion_popup.as_ref().expect("popup");
        assert_eq!(popup.folder, Path::new("/home/me/docs"));
        assert_eq!(t.fake.jobs(), [Job::ApplyExclusions], "first popup was committed");
        assert!(rules(&t).contains(&ExclusionRule::new("/home/me/proj/node_modules", RuleKind::Path)));
        assert!(rules(&t).contains(&ExclusionRule::new("/home/me/docs", RuleKind::Path)));
    }

    #[test]
    fn stale_previews_are_ignored() {
        let mut t = TestApp::new();
        open_popup(&mut t);
        t.fake.send(BackendEvent::ExclusionPreview(ExclusionPreview {
            request: 999,
            entries: 12345,
            folders: 1,
            error: None,
        }));
        t.pump();
        let popup = t.app.windows.exclusion_popup.as_ref().expect("popup");
        assert_eq!(popup.preview.as_ref().expect("preview").entries, 3);
    }

    #[test]
    fn editor_changes_are_saved_and_offer_reindexing() {
        let mut t = TestApp::new();
        t.app.open_exclusion_editor();
        let mut edited = t.app.config.exclusions.clone();
        let removed = edited.remove(0);
        t.app.set_exclusion_rules(edited);
        assert!(t.fake.last_saved_config().is_some());
        assert!(
            !t.fake
                .last_settings()
                .expect("settings")
                .exclusions
                .contains(&removed)
        );
        assert_eq!(t.app.pending_reindex_jobs(), [Job::RescanAll(ScanMode::Full)]);

        t.fake.clear();
        t.app.run_pending_reindex();
        assert_eq!(t.fake.jobs(), [Job::RescanAll(ScanMode::Full)]);
        assert!(t.app.pending_reindex_jobs().is_empty(), "offer handled");
    }

    #[test]
    fn disabling_a_path_rule_offers_a_folder_rescan() {
        let mut t = TestApp::new();
        t.app.config.exclusions = vec![ExclusionRule::new("/mnt/old", RuleKind::Path)];
        t.app.open_exclusion_editor();
        let mut edited = t.app.config.exclusions.clone();
        edited[0].enabled = false;
        t.app.set_exclusion_rules(edited);
        assert_eq!(
            t.app.pending_reindex_jobs(),
            [Job::RescanPath {
                path: "/mnt/old".into(),
                mode: ScanMode::Full
            }]
        );
    }

    #[test]
    fn restore_defaults_and_import_export() {
        let mut t = TestApp::new();
        t.app.open_exclusion_editor();
        t.app
            .set_exclusion_rules(vec![ExclusionRule::new("x", RuleKind::Dir)]);
        t.app.restore_default_exclusions();
        assert_eq!(t.app.config.exclusions, default_rules());

        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("rules.json");
        t.app.windows.exclusion_editor.transfer_path = file.to_string_lossy().into_owned();
        t.app.export_exclusions();
        t.pump();
        let message = t.app.windows.exclusion_editor.message.clone().expect("message");
        assert!(message.starts_with("Exported"), "{message}");

        t.app
            .set_exclusion_rules(vec![ExclusionRule::new("only", RuleKind::Dir)]);
        t.app.import_exclusions();
        t.pump();
        assert_eq!(t.app.config.exclusions.len(), 1 + default_rules().len());
        assert!(
            t.app
                .windows
                .exclusion_editor
                .message
                .clone()
                .expect("message")
                .starts_with("Imported")
        );

        t.app.windows.exclusion_editor.transfer_path =
            dir.path().join("missing.json").to_string_lossy().into_owned();
        t.app.import_exclusions();
        t.pump();
        assert!(
            t.app
                .windows
                .exclusion_editor
                .message
                .clone()
                .expect("message")
                .starts_with("Import failed")
        );
        assert!(
            t.fake
                .calls()
                .iter()
                .any(|c| matches!(c, Call::Compute("import_rules")))
        );
    }
}

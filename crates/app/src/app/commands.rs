//! Commands triggered from menus, global shortcuts and the keyboard.

use everythux_core::config::{ColumnId, SizeFormat, Theme};
use everythux_index::{Job, ScanMode};

use super::interact::{CopyKind, Nav};
use super::{App, PopupId};
use crate::status::ToastKind;

/// Something the user asked for, independent of how (menu, shortcut, button).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    /// `Ctrl+Shift+R`: rebuild the index (asks first if a job is running).
    FullReindex,
    /// "Restart" in the confirmation dialog.
    ConfirmFullReindex,
    /// `F5`.
    QuickRescan,
    CancelJob,
    TogglePreview,
    FocusSearch,
    ToggleMatchPath,
    ToggleCaseSensitive,
    ToggleRegex,
    Quit,
    OpenVolumes,
    OpenExclusions,
    ShowShortcuts,
    ShowAbout,
    /// `Escape`: close the topmost popup, else clear the search box.
    Escape,
    /// `Enter`.
    Open,
    /// `Ctrl+Enter`.
    ShowInFolder,
    /// `F2`: rename the selected entry inline.
    Rename,
    Copy(CopyKind),
    Navigate(Nav, bool),
    ToggleColumn(ColumnId),
    SetTheme(Theme),
    SetSizeFormat(SizeFormat),
}

impl App {
    /// Executes a command.
    pub(crate) fn run_command(&mut self, command: Command) {
        match command {
            Command::FullReindex => {
                if self.index.is_running() {
                    self.windows.confirm_reindex = true;
                    self.open_popup(PopupId::ConfirmReindex);
                } else {
                    self.start_full_reindex();
                }
            }
            Command::ConfirmFullReindex => {
                self.close_popup(PopupId::ConfirmReindex);
                self.backend.cancel_job();
                self.start_full_reindex();
            }
            Command::QuickRescan => {
                self.backend.submit_job(Job::RescanAll(ScanMode::Quick));
                self.toast("Quick rescan started", ToastKind::Info);
            }
            Command::CancelJob => self.backend.cancel_job(),
            Command::TogglePreview => self.toggle_preview_pane(),
            Command::FocusSearch => self.search.focus_requested = true,
            Command::ToggleMatchPath => self.toggle_match_path(),
            Command::ToggleCaseSensitive => self.toggle_case_sensitive(),
            Command::ToggleRegex => self.toggle_regex(),
            Command::Quit => {
                self.quit_requested = true;
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Command::OpenVolumes => self.open_volumes_window(),
            Command::OpenExclusions => self.open_exclusion_editor(),
            Command::ShowShortcuts => {
                self.windows.shortcuts = true;
                self.open_popup(PopupId::Shortcuts);
            }
            Command::ShowAbout => {
                self.windows.about = true;
                self.open_popup(PopupId::About);
            }
            Command::Escape => {
                if !self.close_topmost_popup() && !self.search.text.is_empty() {
                    self.set_search_text("");
                }
            }
            Command::Open => {
                let effects = self.open_selection();
                self.apply_effects(effects);
            }
            Command::ShowInFolder => {
                let effects = self.show_selection_in_folder();
                self.apply_effects(effects);
            }
            Command::Rename => self.rename_selected(),
            Command::Copy(kind) => {
                let effects = self.copy_selection(kind);
                self.apply_effects(effects);
            }
            Command::Navigate(nav, extend) => self.navigate(nav, extend),
            Command::ToggleColumn(id) => self.toggle_column(id),
            Command::SetTheme(theme) => self.set_theme(theme),
            Command::SetSizeFormat(format) => self.set_size_format(format),
        }
    }

    fn start_full_reindex(&mut self) {
        self.backend.submit_job(Job::FullReindex);
        self.toast("Full reindex started", ToastKind::Info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::TestApp;
    use crate::backend::fake::Call;
    use everythux_index::{IndexEvent, JobId};

    use crate::backend::BackendEvent;

    fn start_job(t: &mut TestApp) {
        t.fake.send(BackendEvent::Index(IndexEvent::Started {
            job: JobId(1),
            description: "Indexing /home".into(),
        }));
        t.pump();
    }

    #[test]
    fn full_reindex_submits_immediately_when_idle() {
        let mut t = TestApp::new();
        t.app.run_command(Command::FullReindex);
        assert_eq!(t.fake.jobs(), [Job::FullReindex]);
        assert!(!t.app.windows.confirm_reindex);
    }

    #[test]
    fn full_reindex_while_running_asks_first() {
        let mut t = TestApp::new();
        start_job(&mut t);
        t.app.run_command(Command::FullReindex);
        assert!(t.fake.jobs().is_empty(), "nothing submitted before confirmation");
        assert!(t.app.windows.confirm_reindex);

        t.app.run_command(Command::ConfirmFullReindex);
        let calls = t.fake.calls();
        let cancel = calls
            .iter()
            .position(|c| *c == Call::CancelJob)
            .expect("cancelled");
        let submit = calls
            .iter()
            .position(|c| *c == Call::SubmitJob(Job::FullReindex))
            .expect("submitted");
        assert!(cancel < submit, "cancel first, then restart");
        assert!(!t.app.windows.confirm_reindex);
    }

    #[test]
    fn escape_closes_the_confirmation_before_touching_the_search() {
        let mut t = TestApp::new();
        t.app.set_search_text("abc");
        start_job(&mut t);
        t.app.run_command(Command::FullReindex);
        t.app.run_command(Command::Escape);
        assert!(!t.app.windows.confirm_reindex);
        assert_eq!(t.app.search.text, "abc");
        t.app.run_command(Command::Escape);
        assert_eq!(t.app.search.text, "", "second Escape clears the search");
    }

    #[test]
    fn f5_requests_a_quick_rescan() {
        let mut t = TestApp::new();
        t.app.run_command(Command::QuickRescan);
        assert_eq!(t.fake.jobs(), [Job::RescanAll(ScanMode::Quick)]);
    }

    #[test]
    fn focus_and_toggles() {
        let mut t = TestApp::new();
        t.app.search.focus_requested = false;
        t.app.run_command(Command::FocusSearch);
        assert!(t.app.search.focus_requested);
        t.app.run_command(Command::TogglePreview);
        assert!(t.app.config.ui.preview_pane.enabled);
        t.app.run_command(Command::TogglePreview);
        assert!(!t.app.config.ui.preview_pane.enabled);
    }
}

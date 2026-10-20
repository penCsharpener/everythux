//! Inline rename: starting the editor, committing, and following the renamed entry.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use everythux_core::config::ColumnId;
use everythux_index::Job;

use super::App;
use crate::backend::PlatformAction;
use crate::model::ResultsView;
use crate::rename::{FocusRequest, Message, RenameEdit, Verdict};
use crate::status::{ToastKind, ToastUndo};

/// A click on the selected row's Name cell renames only if the row was selected at least this
/// long ago, in seconds.
pub(crate) const SLOW_CLICK_AFTER: f64 = 0.5;
/// How long a qualifying slow click waits for a second click (a double-click opens the file).
const DOUBLE_CLICK_WAIT: f64 = 0.4;
/// How long the app keeps looking for the renamed entry in new results, in seconds.
const RESELECT_SECONDS: f64 = 15.0;

/// The entry to select once a snapshot containing it arrives.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PendingReselect {
    pub(crate) path: PathBuf,
    /// `egui` time after which the app gives up.
    pub(crate) until: f64,
}

/// What made the editor try to commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitTrigger {
    Enter,
    Tab,
    /// The editor lost focus (a click elsewhere).
    FocusLost,
    /// The editor's row scrolled out of view or vanished.
    Hidden,
}

impl App {
    /// `F2`: renames the selected entry.
    pub(crate) fn rename_selected(&mut self) {
        if self.rename.is_some() {
            return;
        }
        let rows: Vec<usize> = self.selection.rows().take(2).collect();
        match rows.as_slice() {
            [row] => {
                self.start_rename(*row);
            }
            [_, _] => self.toast("Select a single item to rename it", ToastKind::Info),
            _ => {}
        }
    }

    /// Opens the editor in the Name cell of `row`. Returns whether it opened.
    pub(crate) fn start_rename(&mut self, row: usize) -> bool {
        let Some(view) = &self.results else {
            return false;
        };
        let (Some(path), Some(name)) = (view.full_path(row), view.name_bytes(row)) else {
            return false;
        };
        if view.is_offline(row) {
            self.toast(
                "Can't rename: the volume of this item is offline",
                ToastKind::Info,
            );
            return false;
        }
        let name_visible = self
            .config
            .ui
            .columns
            .iter()
            .any(|c| c.id == ColumnId::Name && c.visible);
        if !name_visible {
            self.toast("Show the Name column to rename items", ToastKind::Info);
            return false;
        }
        self.rename = Some(RenameEdit::new(path, row, name.to_vec(), view.is_dir(row)));
        self.slow_click = None;
        self.scroll_to_row = Some(row);
        true
    }

    /// Leaves the editor without doing anything.
    pub(crate) fn cancel_rename(&mut self) {
        self.rename = None;
    }

    /// Tries to commit the editor's text.
    pub(crate) fn commit_rename(&mut self, trigger: CommitTrigger) {
        let Some(edit) = self.rename.as_ref() else {
            return;
        };
        let verdict = edit.verdict();
        let confirmed = edit.warned_for.as_deref() == Some(edit.buffer.as_str())
            && matches!(trigger, CommitTrigger::Enter | CommitTrigger::Tab);
        let buffer = edit.buffer.clone();
        match verdict {
            Verdict::Unchanged => self.rename = None,
            Verdict::Rename(name) => self.submit_rename(name),
            // The editor can't be shown any more, so there is nobody to ask.
            _ if trigger == CommitTrigger::Hidden => self.rename = None,
            Verdict::Confirm(_) if confirmed => self.submit_rename(buffer.into()),
            Verdict::Invalid(error) => self.keep_editing(Message::Error(error), None),
            Verdict::Confirm(warning) => self.keep_editing(
                Message::Warning(format!("{warning} Press Enter again to confirm.")),
                Some(buffer),
            ),
        }
    }

    /// Refuses a commit: shows `message`, remembers which text it warned about, refocuses.
    fn keep_editing(&mut self, message: Message, warned_for: Option<String>) {
        if let Some(edit) = &mut self.rename {
            edit.message = Some(message);
            edit.warned_for = warned_for;
            edit.focus = FocusRequest::Refocus;
        }
    }

    /// Closes the editor and queues the rename on the platform worker.
    fn submit_rename(&mut self, new_name: std::ffi::OsString) {
        let Some(edit) = self.rename.take() else {
            return;
        };
        self.backend.platform(PlatformAction::Rename {
            from: edit.path,
            new_name,
            undo: false,
        });
    }

    /// The platform worker finished a rename.
    pub(crate) fn on_renamed(
        &mut self,
        from: &Path,
        new_name: &OsStr,
        undo: bool,
        result: Result<PathBuf, String>,
    ) {
        let shown = new_name.to_string_lossy().into_owned();
        match result {
            Ok(to) => {
                // Update the index right away; the watcher's later event finds nothing to do.
                // With automatic indexing off, the folder is only noted as out of sync.
                if !self.defer_renamed(from, &to) {
                    self.backend.submit_job(Job::Renamed {
                        from: from.to_path_buf(),
                        to: to.clone(),
                    });
                }
                self.pending_reselect = Some(PendingReselect {
                    path: to.clone(),
                    until: self.now + RESELECT_SECONDS,
                });
                match from.file_name() {
                    Some(previous) if !undo => self.toast_with_undo(
                        format!("Renamed to {shown}"),
                        ToastUndo::Rename {
                            current: to,
                            restore: previous.to_os_string(),
                        },
                    ),
                    _ if undo => self.toast(format!("Renamed back to {shown}"), ToastKind::Info),
                    _ => self.toast(format!("Renamed to {shown}"), ToastKind::Info),
                }
            }
            Err(message) => {
                tracing::warn!(from = %from.display(), %message, "rename failed");
                let old = from
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                self.toast(
                    format!("Could not rename {old} to {shown}: {message}"),
                    ToastKind::Error,
                );
            }
        }
    }

    /// The "Undo" button of a rename toast: renames back through the same path.
    pub(crate) fn undo_rename(&mut self, current: PathBuf, restore: std::ffi::OsString) {
        self.backend.platform(PlatformAction::Rename {
            from: current,
            new_name: restore,
            undo: true,
        });
    }

    /// After new results: keeps the editor on its entry (rows can move), or drops it if the entry
    /// is gone. Then selects the renamed entry if it is listed.
    pub(crate) fn follow_rename_in(&mut self, view: &ResultsView) {
        if let Some(edit) = &mut self.rename {
            if view.full_path(edit.row).as_deref() == Some(edit.path.as_path()) {
                // Still in the same row.
            } else if let Some(row) = view.find_path(&edit.path) {
                edit.row = row;
            } else {
                self.rename = None;
            }
        }
        self.selected_since = None;
        self.slow_click = None;
        let Some(pending) = &self.pending_reselect else {
            return;
        };
        if self.now > pending.until {
            self.pending_reselect = None;
        } else if let Some(row) = view.find_path(&pending.path) {
            self.selection = crate::selection::Selection::from_rows([row], Some(row));
            self.scroll_to_row = Some(row);
            self.pending_reselect = None;
        }
    }

    /// Starts a rename that a slow click asked for once the double-click interval has passed.
    pub(crate) fn tick_slow_click(&mut self) {
        let Some((row, at)) = self.slow_click else {
            return;
        };
        let wait = at + DOUBLE_CLICK_WAIT - self.now;
        if wait > 0.0 {
            self.ctx
                .request_repaint_after(std::time::Duration::from_secs_f64(wait + 0.02));
            return;
        }
        self.slow_click = None;
        if self.selection.len() == 1 && self.selection.contains(row) {
            self.start_rename(row);
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::Modifiers;
    use everythux_core::ids::EntryId;

    use super::*;
    use crate::app::tests::TestApp;
    use crate::app::{CellEvent, ContextAction};
    use crate::backend::BackendEvent;
    use crate::testkit::SnapshotBuilder;

    const NOTES: &str = "/home/me/docs/notes.txt";
    const TODO: &str = "/home/me/docs/todo.txt";

    fn setup() -> TestApp {
        let mut t = TestApp::new();
        t.show_results(&[t.fixture.report, t.fixture.notes, t.fixture.junk]);
        t.app
            .on_cell_event(1, ColumnId::Name, CellEvent::Click(Modifiers::NONE));
        t
    }

    fn renamed_ok(t: &mut TestApp) {
        t.fake.send(BackendEvent::Renamed {
            from: NOTES.into(),
            new_name: "todo.txt".into(),
            undo: false,
            result: Ok(TODO.into()),
        });
        t.pump();
    }

    /// The docs folder after the rename: `todo.txt` replaces `notes.txt`.
    fn snapshot_after_rename() -> (
        std::sync::Arc<everythux_core::snapshot::IndexSnapshot>,
        Vec<EntryId>,
    ) {
        let mut b = SnapshotBuilder::new();
        let home = b.dir("/home/me", None);
        let docs = b.dir("/home/me/docs", Some(home));
        let report = b.file(docs, "report.pdf", 2048, 1);
        let todo = b.file(docs, "todo.txt", 10, 2);
        let other = b.file(docs, "zzz.txt", 10, 2);
        (b.finish(), vec![other, todo, report])
    }

    #[test]
    fn successful_rename_submits_index_job_and_reselects_new_path() {
        let mut t = setup();
        renamed_ok(&mut t);
        assert_eq!(
            t.fake.jobs(),
            [Job::Renamed {
                from: NOTES.into(),
                to: TODO.into()
            }]
        );
        assert_eq!(
            t.app.pending_reselect.as_ref().map(|p| p.path.clone()),
            Some(PathBuf::from(TODO))
        );
        let toast = t.app.toasts.last().expect("toast");
        assert_eq!(toast.text, "Renamed to todo.txt");
        assert!(toast.undo.is_some(), "with an Undo button");

        // The indexer publishes a snapshot; the search service re-runs the same query on it and
        // the renamed entry now sorts elsewhere.
        let (snap, ids) = snapshot_after_rename();
        let generation = t.app.search.latest_generation;
        t.fake.send_results(generation, &snap, &ids, None);
        t.pump();
        assert_eq!(
            t.app.selection.rows().collect::<Vec<_>>(),
            [1],
            "the renamed entry is selected at its new row"
        );
        assert_eq!(t.app.scroll_to_row, Some(1));
        assert_eq!(t.app.pending_reselect, None, "consumed");
    }

    #[test]
    fn reselect_waits_for_a_snapshot_that_contains_the_new_path() {
        let mut t = setup();
        renamed_ok(&mut t);
        // Results of the old snapshot arrive first (a different order, so they are not "unchanged").
        let generation = t.app.search.latest_generation;
        t.fake.send_results(
            generation,
            &t.fixture.snapshot,
            &[t.fixture.junk, t.fixture.report],
            None,
        );
        t.pump();
        assert!(t.app.pending_reselect.is_some(), "still waiting");
        let (snap, ids) = snapshot_after_rename();
        t.fake.send_results(generation, &snap, &ids, None);
        t.pump();
        assert_eq!(t.app.selection.rows().collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn reselect_gives_up_after_a_while() {
        let mut t = setup();
        renamed_ok(&mut t);
        t.app.now += RESELECT_SECONDS + 1.0;
        let generation = t.app.search.latest_generation;
        t.fake.send_results(
            generation,
            &t.fixture.snapshot,
            &[t.fixture.junk, t.fixture.report],
            None,
        );
        t.pump();
        assert_eq!(t.app.pending_reselect, None);
    }

    #[test]
    fn failed_rename_shows_error_and_keeps_name() {
        let mut t = setup();
        t.fake.send(BackendEvent::Renamed {
            from: NOTES.into(),
            new_name: "todo.txt".into(),
            undo: false,
            result: Err("a file or folder with that name already exists".into()),
        });
        t.pump();
        let toast = t.app.toasts.last().expect("toast");
        assert_eq!(toast.kind, ToastKind::Error);
        assert!(
            toast
                .text
                .contains("a file or folder with that name already exists"),
            "{}",
            toast.text
        );
        assert!(toast.text.contains("notes.txt"), "{}", toast.text);
        assert!(toast.undo.is_none());
        assert!(t.fake.jobs().is_empty(), "the index is untouched");
        assert_eq!(t.app.pending_reselect, None);
        assert_eq!(
            t.app.selection.rows().collect::<Vec<_>>(),
            [1],
            "the selection stays on the unchanged entry"
        );
        assert_eq!(
            t.app.results.as_ref().and_then(|r| r.name_bytes(1)),
            Some(b"notes.txt".as_slice())
        );
    }

    #[test]
    fn undo_renames_back_through_the_same_path() {
        let mut t = setup();
        renamed_ok(&mut t);
        let undo = t.app.toasts.last().and_then(|x| x.undo.clone()).expect("undo");
        t.app.undo_toast(undo);
        assert_eq!(
            t.fake.platform_actions(),
            [PlatformAction::Rename {
                from: TODO.into(),
                new_name: "notes.txt".into(),
                undo: true
            }]
        );
        t.fake.send(BackendEvent::Renamed {
            from: TODO.into(),
            new_name: "notes.txt".into(),
            undo: true,
            result: Ok(NOTES.into()),
        });
        t.pump();
        assert_eq!(
            t.fake.jobs().last(),
            Some(&Job::Renamed {
                from: TODO.into(),
                to: NOTES.into()
            })
        );
        let toast = t.app.toasts.last().expect("toast");
        assert_eq!(toast.text, "Renamed back to notes.txt");
        assert!(toast.undo.is_none(), "no undo of an undo");
    }

    #[test]
    fn offline_entries_cannot_be_renamed() {
        let mut t = TestApp::new();
        let mut snap = (*t.fixture.snapshot).clone();
        snap.volumes[0].state = everythux_core::model::VolumeState::Offline;
        let snap = std::sync::Arc::new(snap);
        let generation = t.app.search.latest_generation;
        t.fake.send_results(generation, &snap, &[t.fixture.notes], None);
        t.pump();
        assert!(!t.app.start_rename(0));
        t.app.selection = crate::selection::Selection::from_rows([0], Some(0));
        t.app.rename_selected();
        t.app.context_effects(0, ContextAction::Rename);
        assert!(t.app.rename.is_none());
        assert!(t.app.toasts.last().is_some_and(|x| x.text.contains("offline")));
    }

    #[test]
    fn the_editor_is_dropped_when_its_entry_disappears() {
        let mut t = setup();
        assert!(t.app.start_rename(1));
        let generation = t.app.search.latest_generation;
        t.fake.send_results(
            generation,
            &t.fixture.snapshot,
            &[t.fixture.report, t.fixture.junk],
            None,
        );
        t.pump();
        assert!(t.app.rename.is_none());
    }

    #[test]
    fn slow_click_rename_is_off_by_default() {
        let mut t = setup();
        assert!(!t.app.config.ui.rename_on_slow_click);
        t.app.now = 10.0;
        t.app
            .on_cell_event(1, ColumnId::Name, CellEvent::Click(Modifiers::NONE));
        t.app.now = 11.0;
        t.app.tick_slow_click();
        assert!(t.app.rename.is_none());
        assert_eq!(t.app.slow_click, None);
    }

    #[test]
    fn slow_click_starts_a_rename_when_enabled() {
        let mut t = TestApp::new();
        t.app.config.ui.rename_on_slow_click = true;
        t.show_results(&[t.fixture.report, t.fixture.notes]);
        let click = |t: &mut TestApp, at: f64, row: usize, column: ColumnId| {
            t.app.now = at;
            t.app
                .on_cell_event(row, column, CellEvent::Click(Modifiers::NONE));
        };
        click(&mut t, 1.0, 1, ColumnId::Name); // selects
        click(&mut t, 1.2, 1, ColumnId::Name); // too soon: part of a double-click
        t.app.now = 2.0;
        t.app.tick_slow_click();
        assert!(t.app.rename.is_none(), "a quick second click is not a slow click");

        click(&mut t, 3.0, 1, ColumnId::Path);
        assert!(t.app.slow_click.is_none(), "only the Name cell counts");
        click(&mut t, 4.0, 1, ColumnId::Name); // 3 s after the selection
        assert!(t.app.slow_click.is_some(), "waits out the double-click interval");
        assert!(t.app.rename.is_none());
        t.app.now = 4.1;
        t.app.tick_slow_click();
        assert!(t.app.rename.is_none(), "still waiting");
        t.app.now = 4.6;
        t.app.tick_slow_click();
        assert_eq!(t.app.rename.as_ref().map(|e| e.row), Some(1));
    }

    #[test]
    fn a_double_click_cancels_a_pending_slow_click() {
        let mut t = TestApp::new();
        t.app.config.ui.rename_on_slow_click = true;
        t.show_results(&[t.fixture.report, t.fixture.notes]);
        t.app.now = 1.0;
        t.app
            .on_cell_event(1, ColumnId::Name, CellEvent::Click(Modifiers::NONE));
        t.app.now = 5.0;
        t.app
            .on_cell_event(1, ColumnId::Name, CellEvent::Click(Modifiers::NONE));
        assert!(t.app.slow_click.is_some());
        t.app.now = 5.2;
        let fx = t
            .app
            .on_cell_event(1, ColumnId::Name, CellEvent::DoubleClick(Modifiers::NONE));
        assert_eq!(fx.len(), 1, "the file is opened");
        t.app.now = 6.0;
        t.app.tick_slow_click();
        assert!(t.app.rename.is_none());
    }
}

//! Automatic versus manual indexing: the folders the app's own file operations leave out of sync
//! while automatic indexing is off, and the command that brings the index up to date.

use std::path::{Path, PathBuf};

use everythux_core::dirty::DirtyFolders;
use everythux_index::{Job, ScanMode};

use super::App;
use crate::keys::describe_shortcut;
use crate::status::ToastKind;

impl App {
    /// Whether the indexer keeps the index current on its own (the default).
    pub(crate) fn automatic_indexing(&self) -> bool {
        self.config.indexer.automatic
    }

    /// Runs `record` on the out-of-sync set and persists the set when it changed. The write is
    /// debounced on a worker thread; nothing here touches the disk.
    fn record_dirty(&mut self, record: impl FnOnce(&mut DirtyFolders) -> bool) {
        if record(&mut self.dirty) {
            self.backend.save_dirty_folders(&self.dirty);
        }
    }

    /// Forgets all out-of-sync folders (a rescan that covers them was queued).
    pub(crate) fn clear_dirty(&mut self) {
        if !self.dirty.is_empty() {
            self.dirty.clear();
            self.backend.save_dirty_folders(&self.dirty);
        }
    }

    /// Files at `paths` were deleted or trashed. In manual mode their folders become out of sync
    /// and the function returns `true`; in automatic mode it does nothing and returns `false`,
    /// so the caller queues its rescan as before.
    pub(crate) fn defer_removed(&mut self, paths: &[&Path]) -> bool {
        if self.automatic_indexing() {
            return false;
        }
        self.record_dirty(|dirty| paths.iter().fold(false, |any, p| dirty.note_removed(p) | any));
        true
    }

    /// Items were pasted: `moved` pairs a cut source with its new path, `copied` lists new copies.
    /// Same contract as [`Self::defer_removed`].
    pub(crate) fn defer_pasted(&mut self, moved: &[(&Path, &Path)], copied: &[&Path]) -> bool {
        if self.automatic_indexing() {
            return false;
        }
        self.record_dirty(|dirty| {
            let mut changed = false;
            for (from, to) in moved {
                // The kind is unknown here (no disk access on the UI thread): the parents are
                // enough, because a refresh lists a folder it has not seen before in full.
                changed |= dirty.note_moved(from, to, false);
            }
            for to in copied {
                changed |= dirty.note_added(to);
            }
            changed
        });
        true
    }

    /// `from` was renamed to `to`. Same contract as [`Self::defer_removed`].
    pub(crate) fn defer_renamed(&mut self, from: &Path, to: &Path) -> bool {
        if self.automatic_indexing() {
            return false;
        }
        // A folder shown in the list is known to be one; recording it itself is then cheap.
        let to_is_dir = self
            .results
            .as_ref()
            .and_then(|r| r.find_path(from).map(|row| r.is_dir(row)))
            .unwrap_or(false);
        self.record_dirty(|dirty| dirty.note_moved(from, to, to_is_dir));
        true
    }

    /// `Ctrl+Shift+R` in manual mode (and the Index → Update index entry in both): re-lists the
    /// folders that are out of sync as one job, or, with none recorded, quick-rescans all volumes
    /// so the command always does something useful.
    pub(crate) fn update_index(&mut self) {
        if self.dirty.is_empty() {
            self.backend.submit_job(Job::RescanAll(ScanMode::Quick));
            self.toast(
                "No folders out of sync: quick rescan of all volumes started",
                ToastKind::Info,
            );
            return;
        }
        let folders: Vec<PathBuf> = self.dirty.to_vec();
        let count = folders.len();
        self.backend.submit_job(Job::RefreshDirs(folders));
        self.clear_dirty();
        self.toast(
            format!("Updating the index: {}", folder_count(count)),
            ToastKind::Info,
        );
    }

    /// Switches automatic indexing on or off, now: the indexer starts or stops its watcher and
    /// timers without a restart, and the choice is saved.
    pub(crate) fn set_automatic_indexing(&mut self, automatic: bool) {
        if self.config.indexer.automatic == automatic {
            return;
        }
        self.config.indexer.automatic = automatic;
        self.settings_changed();
        if automatic {
            // Changes made while it was off were not watched: catch up once.
            self.backend.submit_job(Job::RescanAll(ScanMode::Quick));
            self.clear_dirty();
            self.toast(
                "Automatic indexing on: catching up with a quick rescan",
                ToastKind::Info,
            );
        } else {
            self.toast(
                "Automatic indexing off: update the index with Index → Update index",
                ToastKind::Info,
            );
        }
    }

    /// The status-bar note of manual mode: `None` unless indexing is manual and some folders are
    /// out of sync (an up-to-date manual index shows nothing).
    pub(crate) fn manual_index_text(&self) -> Option<String> {
        if self.automatic_indexing() || self.dirty.is_empty() {
            return None;
        }
        let (_, update) = self.bindings.index_shortcuts(false);
        let hint = update.map_or_else(|| "Index → Update index".to_owned(), describe_shortcut);
        Some(format!(
            "Index: manual, {} out of sync ({hint} to update)",
            folder_count(self.dirty.len())
        ))
    }
}

fn folder_count(count: usize) -> String {
    match count {
        1 => "1 folder".to_owned(),
        n => format!("{n} folders"),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    use super::*;
    use crate::app::tests::TestApp;
    use crate::backend::BackendEvent;
    use crate::backend::fake::Call;

    fn manual() -> TestApp {
        let mut t = TestApp::new();
        t.app.config.indexer.automatic = false;
        t.fake.clear();
        t
    }

    fn paths(set: &DirtyFolders) -> Vec<PathBuf> {
        set.to_vec()
    }

    #[test]
    fn automatic_mode_keeps_rescanning_the_folders_and_records_nothing() {
        let mut t = TestApp::new();
        t.fake.clear();
        t.fake.send(BackendEvent::Trashed(vec![(
            "/home/me/docs/notes.txt".into(),
            Ok(()),
        )]));
        t.pump();
        assert_eq!(
            t.fake.jobs(),
            [Job::RescanPath {
                path: "/home/me/docs".into(),
                mode: ScanMode::Quick
            }]
        );
        assert!(t.app.dirty.is_empty());
        assert!(t.fake.last_saved_dirty().is_none(), "nothing is written");
    }

    #[test]
    fn deleting_and_trashing_only_records_the_folders_in_manual_mode() {
        let mut t = manual();
        t.fake.send(BackendEvent::Trashed(vec![
            ("/home/me/docs/a.txt".into(), Ok(())),
            ("/home/me/docs/b.txt".into(), Ok(())),
            ("/home/me/proj/c.txt".into(), Err("denied".into())),
        ]));
        t.fake.send(BackendEvent::Deleted(vec![(
            "/home/me/other/d.txt".into(),
            Ok(()),
        )]));
        t.pump();
        assert!(t.fake.jobs().is_empty(), "no index work: {:?}", t.fake.calls());
        assert_eq!(
            paths(&t.app.dirty),
            [PathBuf::from("/home/me/docs"), "/home/me/other".into()],
            "deduplicated, and failures change nothing"
        );
        assert_eq!(t.fake.last_saved_dirty().as_ref(), Some(&t.app.dirty));
        assert!(
            !t.fake.calls().contains(&Call::SetSnapshot),
            "no snapshot reload per operation"
        );
    }

    #[test]
    fn pasting_records_the_destination_and_a_cut_also_its_sources() {
        let mut t = manual();
        t.fake.send(BackendEvent::Pasted {
            dest: "/dest".into(),
            cut: false,
            results: Ok(vec![
                ("/src/a.txt".into(), Ok("/dest/a.txt".into())),
                ("/src/b.txt".into(), Ok("/dest/b (copy).txt".into())),
            ]),
        });
        t.pump();
        assert_eq!(
            paths(&t.app.dirty),
            [PathBuf::from("/dest")],
            "a copy leaves its source alone"
        );

        t.fake.send(BackendEvent::Pasted {
            dest: "/dest2".into(),
            cut: true,
            results: Ok(vec![
                ("/src/c.txt".into(), Ok("/dest2/c.txt".into())),
                ("/src2/d.txt".into(), Err("exists".into())),
            ]),
        });
        t.pump();
        assert_eq!(
            paths(&t.app.dirty),
            [PathBuf::from("/dest"), "/dest2".into(), "/src".into(),],
            "a failed move changes nothing"
        );
        assert_eq!(t.fake.jobs().len(), 0);
    }

    #[test]
    fn non_utf8_folders_are_recorded_byte_for_byte() {
        let mut t = manual();
        let bad = PathBuf::from(OsStr::from_bytes(b"/data/bad\xffdir/f.txt"));
        t.fake.send(BackendEvent::Trashed(vec![(bad, Ok(()))]));
        t.pump();
        assert_eq!(
            paths(&t.app.dirty),
            [PathBuf::from(OsStr::from_bytes(b"/data/bad\xffdir"))]
        );
        let saved = t.fake.last_saved_dirty().expect("saved");
        assert_eq!(DirtyFolders::parse(&saved.to_bytes()), saved);
    }

    #[test]
    fn renaming_a_folder_records_its_parent_and_the_folder_itself() {
        let mut t = manual();
        let ids = [t.fixture.docs_entry, t.fixture.notes];
        t.show_results(&ids);
        t.app.on_renamed(
            "/home/me/docs".as_ref(),
            OsStr::new("papers"),
            false,
            Ok("/home/me/papers".into()),
        );
        assert_eq!(
            paths(&t.app.dirty),
            [PathBuf::from("/home/me"), "/home/me/papers".into()]
        );
        assert!(t.fake.jobs().is_empty(), "no Job::Renamed in manual mode");

        // A file only dirties the folder it is in.
        let mut files = manual();
        files.show_results(&[files.fixture.notes]);
        files.app.on_renamed(
            "/home/me/docs/notes.txt".as_ref(),
            OsStr::new("todo.txt"),
            false,
            Ok("/home/me/docs/todo.txt".into()),
        );
        assert_eq!(paths(&files.app.dirty), [PathBuf::from("/home/me/docs")]);
    }

    #[test]
    fn updating_submits_the_folders_as_one_refresh_and_forgets_them() {
        let mut t = manual();
        t.fake.send(BackendEvent::Trashed(vec![
            ("/z/a.txt".into(), Ok(())),
            ("/a/b.txt".into(), Ok(())),
        ]));
        t.pump();
        t.fake.clear();
        t.app.update_index();
        assert_eq!(t.fake.jobs(), [Job::RefreshDirs(vec!["/a".into(), "/z".into()])]);
        assert!(t.app.dirty.is_empty(), "cleared once the job was handed over");
        assert!(t.fake.last_saved_dirty().expect("persisted").is_empty());
        assert_eq!(t.app.manual_index_text(), None);
    }

    #[test]
    fn updating_with_nothing_out_of_sync_quick_rescans_every_volume() {
        let mut t = manual();
        t.app.update_index();
        assert_eq!(t.fake.jobs(), [Job::RescanAll(ScanMode::Quick)]);
        let toast = t.app.toasts.last().expect("toast");
        assert!(
            toast.text.contains("quick rescan of all volumes"),
            "{}",
            toast.text
        );
    }

    #[test]
    fn a_full_reindex_or_quick_rescan_covers_the_recorded_folders() {
        let mut t = manual();
        t.app.dirty.add_folder("/a".as_ref());
        t.app.run_command(crate::app::Command::QuickRescan);
        assert!(t.app.dirty.is_empty());
        t.app.dirty.add_folder("/a".as_ref());
        t.app.run_command(crate::app::Command::FullReindex);
        assert!(t.app.dirty.is_empty());
    }

    #[test]
    fn the_status_note_appears_only_in_manual_mode_with_folders_out_of_sync() {
        let mut t = manual();
        assert_eq!(t.app.manual_index_text(), None, "nothing to report yet");
        t.app.dirty.add_folder("/a".as_ref());
        assert_eq!(
            t.app.manual_index_text().as_deref(),
            Some("Index: manual, 1 folder out of sync (Ctrl+Shift+R to update)")
        );
        t.app.dirty.add_folder("/b".as_ref());
        assert_eq!(
            t.app.manual_index_text().as_deref(),
            Some("Index: manual, 2 folders out of sync (Ctrl+Shift+R to update)")
        );
        t.app.config.indexer.automatic = true;
        assert_eq!(t.app.manual_index_text(), None);
    }

    #[test]
    fn switching_modes_reaches_the_indexer_and_the_config_at_runtime() {
        let mut t = TestApp::new();
        t.fake.clear();
        t.app.set_automatic_indexing(false);
        assert!(!t.app.config.indexer.automatic);
        assert!(
            !t.fake
                .last_settings()
                .expect("pushed to the indexer")
                .indexer
                .automatic
        );
        assert!(!t.fake.last_saved_config().expect("saved").indexer.automatic);
        assert!(t.fake.jobs().is_empty(), "turning it off starts no job");

        t.app.dirty.add_folder("/a".as_ref());
        t.fake.clear();
        t.app.set_automatic_indexing(true);
        assert!(t.fake.last_settings().expect("pushed").indexer.automatic);
        assert!(t.fake.last_saved_config().expect("saved").indexer.automatic);
        assert_eq!(
            t.fake.jobs(),
            [Job::RescanAll(ScanMode::Quick)],
            "one catch-up rescan for what was not watched"
        );
        assert!(t.app.dirty.is_empty());
    }

    #[test]
    fn setting_the_current_mode_again_does_nothing() {
        let mut t = TestApp::new();
        t.fake.clear();
        t.app.set_automatic_indexing(true);
        assert!(t.fake.calls().is_empty(), "{:?}", t.fake.calls());
    }
}

//! Bookmarked searches: saved in the config, run from the Bookmarks menu.

use everythux_core::config::Bookmark;

use super::{App, Command, PopupId};
use crate::backend::Compute;
use crate::search_bars::FIRST_BAR;

impl App {
    /// The Bookmarks menu commands.
    pub(crate) fn bookmark_command(&mut self, command: &Command) {
        match command {
            Command::AddBookmark => {
                self.windows.bookmarks.name = self
                    .search
                    .texts()
                    .into_iter()
                    .find(|t| !t.trim().is_empty())
                    .unwrap_or_default();
                self.open_bookmarks_window();
            }
            Command::OpenBookmarks => self.open_bookmarks_window(),
            Command::RunBookmark(index) => self.run_bookmark(*index),
            _ => {}
        }
    }

    /// Shows the bookmarks window on top.
    pub(crate) fn open_bookmarks_window(&mut self) {
        self.windows.bookmarks.open = true;
        self.open_popup(PopupId::Bookmarks);
    }

    /// Bookmarks the current search (all bars and the three switches) under `name`; an empty name
    /// falls back to the search text. Blank searches are not bookmarked. Returns whether one was
    /// added.
    pub(crate) fn add_bookmark(&mut self, name: &str) -> bool {
        let Some(texts) = everythux_core::history::normalize(&self.search.texts()) else {
            return false;
        };
        let name = match name.trim() {
            "" => texts.join(" AND "),
            given => given.to_owned(),
        };
        let s = &self.config.search;
        self.config.bookmarks.push(Bookmark {
            name,
            texts,
            case_sensitive: s.case_sensitive,
            match_path: s.match_path,
            regex: s.regex,
        });
        self.config_changed();
        true
    }

    /// Removes bookmark `index`.
    pub(crate) fn delete_bookmark(&mut self, index: usize) {
        if index < self.config.bookmarks.len() {
            self.config.bookmarks.remove(index);
            self.config_changed();
        }
    }

    /// Bookmarks → window → Export: writes all bookmarks to the typed path.
    pub(crate) fn export_bookmarks(&mut self) {
        let path = self.windows.bookmarks.transfer_path.trim().to_owned();
        if path.is_empty() {
            self.windows.bookmarks.message = Some("Enter a file path first".to_owned());
            return;
        }
        self.backend.compute(Compute::ExportBookmarks {
            path: path.into(),
            bookmarks: self.config.bookmarks.clone(),
        });
    }

    /// Bookmarks → window → Import: reads bookmarks from the typed path.
    pub(crate) fn import_bookmarks(&mut self) {
        let path = self.windows.bookmarks.transfer_path.trim().to_owned();
        if path.is_empty() {
            self.windows.bookmarks.message = Some("Enter a file path first".to_owned());
            return;
        }
        self.backend.compute(Compute::ImportBookmarks(path.into()));
    }

    /// Adds the imported bookmarks that are not there yet, at the end.
    pub(crate) fn on_bookmarks_imported(&mut self, result: Result<Vec<Bookmark>, String>) {
        self.windows.bookmarks.message = Some(match result {
            Ok(found) => {
                let total = found.len();
                let before = self.config.bookmarks.len();
                for bookmark in found {
                    if !self.config.bookmarks.contains(&bookmark) {
                        self.config.bookmarks.push(bookmark);
                    }
                }
                let added = self.config.bookmarks.len() - before;
                if added > 0 {
                    self.config_changed();
                }
                format!("Imported {added} new bookmark(s) from {total}")
            }
            Err(e) => format!("Import failed: {e}"),
        });
    }

    pub(crate) fn on_bookmarks_exported(&mut self, result: Result<(std::path::PathBuf, usize), String>) {
        self.windows.bookmarks.message = Some(match result {
            Ok((path, n)) => format!("Exported {n} bookmark(s) to {}", path.display()),
            Err(e) => format!("Export failed: {e}"),
        });
    }

    /// Runs bookmark `index`: restores its bars and switches and searches.
    pub(crate) fn run_bookmark(&mut self, index: usize) {
        let Some(bookmark) = self.config.bookmarks.get(index).cloned() else {
            return;
        };
        self.search.set_texts(&bookmark.texts);
        let s = &mut self.config.search;
        s.case_sensitive = bookmark.case_sensitive;
        s.match_path = bookmark.match_path;
        s.regex = bookmark.regex;
        self.config_changed();
        self.record_history();
        self.search.focus_bar = Some(FIRST_BAR);
        self.resubmit_search();
    }
}

#[cfg(test)]
mod tests {
    use crate::app::tests::TestApp;

    #[test]
    fn bookmarks_keep_the_search_and_its_switches() {
        let mut t = TestApp::new();
        t.app.config.search.case_sensitive = true;
        t.app.set_search_text("report");
        assert!(t.app.add_bookmark(""));
        assert_eq!(t.app.config.bookmarks[0].name, "report");
        assert_eq!(t.fake.last_saved_config().expect("saved").bookmarks.len(), 1);

        t.app.config.search.case_sensitive = false;
        t.app.set_search_text("other");
        t.app.run_bookmark(0);
        assert_eq!(t.app.search.texts(), ["report"]);
        assert!(t.app.config.search.case_sensitive);
        let last = t.fake.searches().last().cloned().expect("search");
        assert_eq!(last.texts, ["report"]);

        t.app.set_search_text("  ");
        assert!(!t.app.add_bookmark("blank"), "blank searches are not bookmarked");
        t.app.delete_bookmark(0);
        assert_eq!(t.app.config.bookmarks, []);
    }

    #[test]
    fn export_then_import_merges_without_duplicates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("b.json");
        let mut t = TestApp::new();
        t.app.set_search_text("report");
        t.app.add_bookmark("reports");
        t.app.windows.bookmarks.transfer_path = file.display().to_string();
        t.app.export_bookmarks();
        t.pump();
        assert!(
            t.app
                .windows
                .bookmarks
                .message
                .as_deref()
                .is_some_and(|m| m.contains("Exported 1")),
            "{:?}",
            t.app.windows.bookmarks.message
        );

        t.app.set_search_text("notes");
        t.app.add_bookmark("notes");
        t.app.import_bookmarks();
        t.pump();
        let names: Vec<&str> = t.app.config.bookmarks.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            ["reports", "notes"],
            "the existing bookmark is not duplicated"
        );

        t.app.config.bookmarks.clear();
        t.app.import_bookmarks();
        t.pump();
        assert_eq!(t.app.config.bookmarks.len(), 1);
    }
}

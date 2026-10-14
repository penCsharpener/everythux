//! Search submission and result handling.

use std::collections::{HashMap, HashSet};
use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;

use everythux_search::{QueryOptions, SearchRequest, SearchResults, Sort};

use super::App;
use crate::model::ResultsView;
use crate::selection::Selection;

/// Most selected rows carried over when the results are re-run against a new snapshot.
const MAX_REMAPPED_ROWS: usize = 256;

impl App {
    /// The request describing the search box and the current switches.
    pub(crate) fn current_request(&self) -> SearchRequest {
        let sort = self.config.ui.sort;
        SearchRequest {
            text: self.search.text.clone(),
            options: QueryOptions::from(&self.config.search),
            sort: Sort {
                column: sort.column,
                ascending: sort.ascending,
            },
            limit: self.config.search.max_results,
        }
    }

    /// Submits the current request and remembers its generation. The service coalesces rapid
    /// submissions, so calling this on every keystroke is fine.
    pub(crate) fn resubmit_search(&mut self) {
        let request = self.current_request();
        self.search.latest_generation = self.backend.search(request);
    }

    /// Replaces the search text (used by Escape and tests) and searches.
    pub(crate) fn set_search_text(&mut self, text: impl Into<String>) {
        self.search.text = text.into();
        self.resubmit_search();
    }

    /// Folds search results into the state; results older than the latest submission are dropped.
    pub(crate) fn on_search_results(&mut self, res: SearchResults) {
        if res.generation < self.search.latest_generation {
            tracing::trace!(
                generation = res.generation,
                latest = self.search.latest_generation,
                "dropping stale search results"
            );
            return;
        }
        self.search.error = res.error;
        let same_request = res.generation == self.search.results_generation;
        self.search.results_generation = res.generation;
        let view = ResultsView {
            ids: res.ids,
            snapshot: res.snapshot,
            total_entries: res.total_entries,
            elapsed: res.elapsed,
        };
        let unchanged = self.results.as_ref().is_some_and(|old| {
            std::sync::Arc::ptr_eq(&old.snapshot, &view.snapshot)
                && (std::sync::Arc::ptr_eq(&old.ids, &view.ids) || *old.ids == *view.ids)
        });
        if !unchanged {
            // A new snapshot with the same query (a background re-index) only moves rows around:
            // keep whatever is still listed selected, so the preview does not vanish under the
            // user. A different query starts from a clean selection.
            if same_request {
                self.selection = self.remapped_selection(&view);
            } else {
                self.selection.clear();
            }
            self.scroll_to_row = None;
            self.cache.invalidate();
            self.follow_rename_in(&view);
        }
        self.results = Some(view);
    }

    /// The current selection translated to the rows of `new`, matching entries by full path.
    /// Entries that are gone are dropped; large selections are not carried over.
    fn remapped_selection(&self, new: &ResultsView) -> Selection {
        let Some(old) = &self.results else {
            return Selection::default();
        };
        let rows: Vec<usize> = self.selection.rows().take(MAX_REMAPPED_ROWS + 1).collect();
        if rows.is_empty() || rows.len() > MAX_REMAPPED_ROWS {
            return Selection::default();
        }
        let cursor_path = self.selection.cursor().and_then(|r| old.full_path(r));
        let wanted: HashSet<PathBuf> = rows
            .iter()
            .filter_map(|&r| old.full_path(r))
            .chain(cursor_path.clone())
            .collect();
        let names: HashSet<Vec<u8>> = wanted
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| n.as_bytes().to_vec())
            .collect();

        // One pass over the new rows; the cheap name comparison runs first and the path is only
        // built for the few rows that share a name with a selected entry.
        let snap = &*new.snapshot;
        let mut found: HashMap<PathBuf, usize> = HashMap::with_capacity(wanted.len());
        for (row, &id) in new.ids.iter().enumerate() {
            if names.contains(snap.name(id)) {
                let path = snap.full_path(id);
                if wanted.contains(&path) {
                    found.insert(path, row);
                    if found.len() == wanted.len() {
                        break;
                    }
                }
            }
        }
        let cursor = cursor_path.and_then(|p| found.get(&p).copied());
        let selected = rows
            .iter()
            .filter_map(|&r| old.full_path(r))
            .filter_map(|p| found.get(&p).copied());
        Selection::from_rows(selected, cursor)
    }
}

#[cfg(test)]
mod tests {
    use everythux_core::config::ColumnId;

    use crate::app::tests::TestApp;

    #[test]
    fn typing_submits_a_request_with_the_current_options() {
        let mut t = TestApp::new();
        t.fake.clear();
        t.app.set_search_text("report");
        let searches = t.fake.searches();
        assert_eq!(searches.len(), 1);
        assert_eq!(searches[0].text, "report");
        assert!(!searches[0].options.regex);
        assert_eq!(searches[0].sort.column, ColumnId::Name);
        assert!(searches[0].sort.ascending);
    }

    #[test]
    fn stale_generations_are_ignored() {
        let mut t = TestApp::new();
        t.app.set_search_text("a");
        let old = t.app.search.latest_generation;
        t.app.set_search_text("ab");
        let new = t.app.search.latest_generation;
        assert!(new > old);

        t.fake
            .send_results(new, &t.fixture.snapshot, &[t.fixture.report], None);
        t.pump();
        assert_eq!(t.app.results.as_ref().expect("results").len(), 1);

        t.fake.send_results(
            old,
            &t.fixture.snapshot,
            &[t.fixture.report, t.fixture.notes],
            None,
        );
        t.pump();
        assert_eq!(
            t.app.results.as_ref().expect("results").len(),
            1,
            "stale results dropped"
        );
    }

    #[test]
    fn parse_errors_keep_old_results_and_show_the_message() {
        let mut t = TestApp::new();
        t.show_all_results();
        let before = t.app.results.as_ref().expect("results").len();
        t.app.set_search_text("(");
        let generation = t.app.search.latest_generation;
        let ids = t.app.results.as_ref().expect("results").ids.clone();
        t.fake.send_results(
            generation,
            &t.fixture.snapshot,
            &ids,
            Some("unbalanced parenthesis"),
        );
        t.pump();
        assert_eq!(t.app.search.error.as_deref(), Some("unbalanced parenthesis"));
        assert_eq!(t.app.results.as_ref().expect("results").len(), before);
        // The next good query clears the error.
        t.app.set_search_text("report");
        let generation = t.app.search.latest_generation;
        t.fake
            .send_results(generation, &t.fixture.snapshot, &[t.fixture.report], None);
        t.pump();
        assert_eq!(t.app.search.error, None);
    }

    #[test]
    fn header_click_toggles_direction_and_resubmits() {
        let mut t = TestApp::new();
        t.fake.clear();
        t.app.sort_by(ColumnId::Size);
        t.app.sort_by(ColumnId::Size);
        t.app.sort_by(ColumnId::Modified);
        let sorts: Vec<_> = t
            .fake
            .searches()
            .iter()
            .map(|s| (s.sort.column, s.sort.ascending))
            .collect();
        assert_eq!(
            sorts,
            [
                (ColumnId::Size, true),
                (ColumnId::Size, false),
                (ColumnId::Modified, true)
            ]
        );
        let saved = t.fake.last_saved_config().expect("saved");
        assert_eq!(saved.ui.sort.column, ColumnId::Modified);
    }

    #[test]
    fn identical_results_keep_the_selection() {
        let mut t = TestApp::new();
        t.show_all_results();
        t.app.selection.select_only(1);
        let generation = t.app.search.latest_generation + 1;
        t.app.search.latest_generation = generation - 1;
        let ids = t.app.results.as_ref().expect("results").ids.clone();
        t.fake.send_results(generation, &t.fixture.snapshot, &ids, None);
        t.pump();
        assert!(
            t.app.selection.contains(1),
            "background refresh must not lose the selection"
        );
    }

    /// Delivers the same query against a freshly built snapshot (a background re-index).
    fn refresh_with_new_snapshot(t: &mut TestApp, names: &[&str]) {
        let fresh = crate::testkit::Fixture::new();
        let ids: Vec<_> = names
            .iter()
            .map(|n| match *n {
                "report.pdf" => fresh.report,
                "notes.txt" => fresh.notes,
                "junk.js" => fresh.junk,
                other => panic!("unknown fixture entry {other}"),
            })
            .collect();
        let generation = t.app.search.latest_generation;
        t.fake.send_results(generation, &fresh.snapshot, &ids, None);
        t.pump();
    }

    #[test]
    fn a_new_snapshot_keeps_the_selected_entry_at_its_new_row() {
        let mut t = TestApp::new();
        t.show_results(&[t.fixture.report, t.fixture.notes, t.fixture.junk]);
        t.app.selection.select_only(1); // notes.txt
        refresh_with_new_snapshot(&mut t, &["junk.js", "notes.txt", "report.pdf"]);
        assert_eq!(t.app.selection.rows().collect::<Vec<_>>(), [1]);
        refresh_with_new_snapshot(&mut t, &["notes.txt", "junk.js"]);
        assert_eq!(
            t.app.selection.rows().collect::<Vec<_>>(),
            [0],
            "followed to row 0"
        );
        assert_eq!(t.app.selection.cursor(), Some(0));
        let wanted = t.app.results.as_ref().expect("results").full_path(0);
        assert_eq!(wanted, Some("/home/me/docs/notes.txt".into()));
    }

    #[test]
    fn a_new_snapshot_without_the_selected_entry_clears_the_selection() {
        let mut t = TestApp::new();
        t.show_results(&[t.fixture.report, t.fixture.notes, t.fixture.junk]);
        t.app.selection.select_only(1);
        refresh_with_new_snapshot(&mut t, &["junk.js", "report.pdf"]);
        assert!(t.app.selection.is_empty());
    }

    #[test]
    fn a_new_snapshot_keeps_a_multi_selection_that_is_still_listed() {
        let mut t = TestApp::new();
        t.show_results(&[t.fixture.report, t.fixture.notes, t.fixture.junk]);
        t.app.selection.select_only(0);
        t.app.selection.toggle(2); // report.pdf + junk.js, cursor on junk.js
        refresh_with_new_snapshot(&mut t, &["junk.js", "notes.txt", "report.pdf"]);
        assert_eq!(t.app.selection.rows().collect::<Vec<_>>(), [0, 2]);
        assert_eq!(t.app.selection.cursor(), Some(0), "cursor stays on junk.js");
    }

    #[test]
    fn a_new_query_clears_the_selection_even_if_the_entry_is_listed_again() {
        let mut t = TestApp::new();
        t.show_results(&[t.fixture.report, t.fixture.notes]);
        t.app.selection.select_only(1);
        t.app.set_search_text("txt");
        refresh_with_new_snapshot(&mut t, &["notes.txt"]);
        assert!(t.app.selection.is_empty());
    }

    #[test]
    fn different_results_clear_the_selection() {
        let mut t = TestApp::new();
        t.show_all_results();
        t.app.selection.select_only(1);
        t.app.set_search_text("x");
        let generation = t.app.search.latest_generation;
        t.fake
            .send_results(generation, &t.fixture.snapshot, &[t.fixture.report], None);
        t.pump();
        assert!(t.app.selection.is_empty());
    }
}

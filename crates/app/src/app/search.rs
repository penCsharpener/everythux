//! Search submission and result handling.

use everythux_search::{QueryOptions, SearchRequest, SearchResults, Sort};

use super::App;
use crate::model::ResultsView;

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
            self.selection.clear();
            self.scroll_to_row = None;
            self.cache.invalidate();
        }
        self.results = Some(view);
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

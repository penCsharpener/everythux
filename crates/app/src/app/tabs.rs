//! Tabs: each one has its own search bars, result list and selection.
//!
//! The active tab's state lives directly in [`App`] (`search`, `results`, `selection`), so the rest
//! of the app never has to know about tabs. The other tabs are parked in `App::tabs`; switching
//! swaps the parked state with the live one and re-runs the search, so the list always matches
//! the current index. The slot of the active tab is an unused placeholder.

use super::App;
use crate::model::ResultsView;
use crate::search_bars::{FIRST_BAR, SearchState};
use crate::selection::Selection;

/// Soft limit on the number of tabs; `Ctrl+T` does nothing beyond it.
pub(crate) const MAX_TABS: usize = 32;

/// The per-tab state of a tab that is not shown.
#[derive(Debug, Default)]
pub(crate) struct TabSlot {
    pub(crate) search: SearchState,
    pub(crate) results: Option<ResultsView>,
    pub(crate) selection: Selection,
}

/// Longest tab label in characters; longer queries are cut with an ellipsis.
const TITLE_CHARS: usize = 24;

impl App {
    /// Number of tabs.
    pub(crate) fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// Label of tab `index`: its combined query, or "New tab" while it is empty.
    pub(crate) fn tab_title(&self, index: usize) -> String {
        let search = if index == self.active_tab {
            &self.search
        } else {
            match self.tabs.get(index) {
                Some(slot) => &slot.search,
                None => return String::new(),
            }
        };
        let query = search.combined_query();
        if query.is_empty() {
            return "New tab".to_owned();
        }
        if query.chars().count() <= TITLE_CHARS {
            return query;
        }
        let mut cut: String = query.chars().take(TITLE_CHARS - 1).collect();
        cut.push('…');
        cut
    }

    /// `Ctrl+T`: opens an empty tab right after the current one and shows it.
    pub(crate) fn new_tab(&mut self) {
        if self.tabs.len() >= MAX_TABS {
            return;
        }
        let at = self.active_tab + 1;
        self.tabs.insert(at, TabSlot::default());
        self.select_tab(at);
    }

    /// Shows tab `index`. The search of the tab is re-run, and its first bar gets the focus.
    pub(crate) fn select_tab(&mut self, index: usize) {
        if index == self.active_tab || index >= self.tabs.len() {
            return;
        }
        self.cancel_rename();
        let mut incoming = std::mem::take(&mut self.tabs[index]);
        std::mem::swap(&mut self.search, &mut incoming.search);
        std::mem::swap(&mut self.results, &mut incoming.results);
        std::mem::swap(&mut self.selection, &mut incoming.selection);
        // `incoming` now holds the tab that was shown.
        self.tabs[self.active_tab] = incoming;
        self.active_tab = index;

        self.pending_reselect = None;
        self.selected_since = None;
        self.slow_click = None;
        self.scroll_to_row = None;
        self.cache.invalidate();
        self.search.shown_title = None;
        self.search.focus_bar = Some(FIRST_BAR);
        self.resubmit_search();
    }

    /// `Ctrl+Tab` / `Ctrl+Shift+Tab`: the next or previous tab, wrapping around.
    pub(crate) fn cycle_tab(&mut self, forward: bool) {
        let n = self.tabs.len();
        let next = if forward {
            (self.active_tab + 1) % n
        } else {
            (self.active_tab + n - 1) % n
        };
        self.select_tab(next);
    }

    /// Closes tab `index` (`Ctrl+W` for the current one). The last tab can't be closed. Closing
    /// the current tab shows its left neighbour, or the new first tab.
    pub(crate) fn close_tab(&mut self, index: usize) {
        if self.tabs.len() <= 1 || index >= self.tabs.len() {
            return;
        }
        if index == self.active_tab {
            let neighbour = if index > 0 { index - 1 } else { 1 };
            self.select_tab(neighbour);
        }
        self.tabs.remove(index);
        if self.active_tab > index {
            self.active_tab -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::tests::TestApp;

    #[test]
    fn a_new_tab_starts_empty_and_keeps_the_old_one() {
        let mut t = TestApp::new();
        t.app.set_search_text("report");
        t.show_all_results();
        t.app.selection.select_only(1);

        t.app.new_tab();
        assert_eq!(t.app.tab_count(), 2);
        assert_eq!(t.app.active_tab, 1);
        assert_eq!(t.app.search.combined_query(), "");
        assert!(t.app.selection.is_empty());
        assert_eq!(t.app.tab_title(0), "report");
        assert_eq!(t.app.tab_title(1), "New tab");
        let last = t.fake.searches().pop().expect("the new tab searched");
        assert_eq!(last.texts, [""]);

        t.app.select_tab(0);
        assert_eq!(t.app.search.first().text, "report");
        assert_eq!(t.app.tab_title(1), "New tab");
    }

    #[test]
    fn closing_tabs_picks_a_neighbour_and_keeps_the_last() {
        let mut t = TestApp::new();
        t.app.set_search_text("a");
        t.app.new_tab();
        t.app.set_search_text("b");
        t.app.new_tab();
        t.app.set_search_text("c");
        assert_eq!(t.app.active_tab, 2);

        t.app.close_tab(2);
        assert_eq!((t.app.tab_count(), t.app.active_tab), (2, 1));
        assert_eq!(t.app.search.first().text, "b");

        t.app.close_tab(0);
        assert_eq!((t.app.tab_count(), t.app.active_tab), (1, 0));
        assert_eq!(t.app.search.first().text, "b");

        t.app.close_tab(0);
        assert_eq!(t.app.tab_count(), 1, "the last tab stays");
    }

    #[test]
    fn cycling_wraps_around() {
        let mut t = TestApp::new();
        t.app.new_tab();
        t.app.new_tab();
        assert_eq!(t.app.active_tab, 2);
        t.app.cycle_tab(true);
        assert_eq!(t.app.active_tab, 0);
        t.app.cycle_tab(false);
        assert_eq!(t.app.active_tab, 2);
    }

    #[test]
    fn long_queries_are_cut() {
        let mut t = TestApp::new();
        t.app.set_search_text("x".repeat(40));
        let title = t.app.tab_title(0);
        assert_eq!(title.chars().count(), 24);
        assert!(title.ends_with('…'));
    }
}

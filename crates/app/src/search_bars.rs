//! The search bars: one or more queries that are combined with AND.
//!
//! Bar 0 always exists. Extra bars are appended below it and live for the session only. Every bar
//! has a stable id, so egui widget ids (and with them focus and cursor state) survive closing a bar
//! in the middle. The texts are sent to the search service one per bar and parsed separately, so
//! a broken quote in one bar can't leak into another.

/// Soft limit on the number of bars; the `+` button is disabled beyond it.
pub(crate) const MAX_SEARCH_BARS: usize = 8;

/// Id of the first bar, which can't be closed.
pub(crate) const FIRST_BAR: u32 = 0;

/// One search bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchBar {
    /// Stable id, never reused within a session.
    pub(crate) id: u32,
    /// Current text.
    pub(crate) text: String,
    /// Parse error of this bar's text in the last query, shown under it.
    pub(crate) error: Option<String>,
}

impl SearchBar {
    fn new(id: u32) -> Self {
        Self {
            id,
            text: String::new(),
            error: None,
        }
    }
}

/// Search box state.
#[derive(Debug)]
pub(crate) struct SearchState {
    /// The bars, top to bottom. Never empty; `bars[0]` is the first bar.
    pub(crate) bars: Vec<SearchBar>,
    next_id: u32,
    /// Generation of the most recently submitted request; older results are ignored.
    pub(crate) latest_generation: u64,
    /// Generation of the request the current results answer. Results with the same generation
    /// are the same query re-run against a newer snapshot.
    pub(crate) results_generation: u64,
    /// Bar to focus on the next frame.
    pub(crate) focus_bar: Option<u32>,
    /// The window title last sent to the viewport.
    pub(crate) shown_title: Option<String>,
}

impl Default for SearchState {
    /// One empty bar, focused on the first frame (the state on startup).
    fn default() -> Self {
        Self {
            bars: vec![SearchBar::new(FIRST_BAR)],
            next_id: FIRST_BAR + 1,
            latest_generation: 0,
            results_generation: 0,
            focus_bar: Some(FIRST_BAR),
            shown_title: None,
        }
    }
}

impl SearchState {
    /// The first bar.
    #[cfg(test)]
    pub(crate) fn first(&self) -> &SearchBar {
        &self.bars[0]
    }

    /// The first bar, mutably.
    #[cfg(test)]
    pub(crate) fn first_mut(&mut self) -> &mut SearchBar {
        &mut self.bars[0]
    }

    /// The bar with `id`, if it is still open.
    pub(crate) fn bar_mut(&mut self, id: u32) -> Option<&mut SearchBar> {
        self.bars.iter_mut().find(|b| b.id == id)
    }

    /// The newest (last, bottom-most) bar, which `Tab` jumps to.
    pub(crate) fn newest(&self) -> u32 {
        self.bars.last().map_or(FIRST_BAR, |b| b.id)
    }

    /// The text of every bar, top to bottom, for the search request.
    pub(crate) fn texts(&self) -> Vec<String> {
        self.bars.iter().map(|b| b.text.clone()).collect()
    }

    /// Whether another bar may be added.
    pub(crate) fn can_add(&self) -> bool {
        self.bars.len() < MAX_SEARCH_BARS
    }

    /// Appends an empty bar and returns its id, or `None` at the limit.
    pub(crate) fn add(&mut self) -> Option<u32> {
        if !self.can_add() {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.bars.push(SearchBar::new(id));
        Some(id)
    }

    /// Removes the extra bar `id` and returns the id of the bar above it. The first bar and
    /// unknown ids are left alone (`None`).
    pub(crate) fn close(&mut self, id: u32) -> Option<u32> {
        let pos = self.bars.iter().position(|b| b.id == id).filter(|&p| p > 0)?;
        self.bars.remove(pos);
        Some(self.bars[pos - 1].id)
    }

    /// Attaches a parse error to the bar at `index` and clears the others; `None` clears all.
    pub(crate) fn set_error(&mut self, error: Option<(usize, String)>) {
        for bar in &mut self.bars {
            bar.error = None;
        }
        if let Some((index, message)) = error {
            // A bar closed since the request was sent has nothing to show it under; the first
            // bar does.
            let index = index.min(self.bars.len() - 1);
            self.bars[index].error = Some(message);
        }
    }

    /// The non-empty bars joined with `AND`, as shown in the status bar and window title.
    /// A bar with several words is parenthesised when there is more than one active bar, so
    /// `(holiday draft) AND ext:jpg` reads unambiguously.
    pub(crate) fn combined_query(&self) -> String {
        let active: Vec<&str> = self
            .bars
            .iter()
            .map(|b| b.text.trim())
            .filter(|t| !t.is_empty())
            .collect();
        if active.len() == 1 {
            return active[0].to_owned();
        }
        let mut out = String::new();
        for (i, text) in active.iter().enumerate() {
            if i > 0 {
                out.push_str(" AND ");
            }
            if text.contains(char::is_whitespace) {
                out.push('(');
                out.push_str(text);
                out.push(')');
            } else {
                out.push_str(text);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(texts: &[&str]) -> SearchState {
        let mut s = SearchState::default();
        s.first_mut().text = texts[0].to_owned();
        for text in &texts[1..] {
            let id = s.add().expect("below the limit");
            s.bar_mut(id).expect("just added").text = (*text).to_owned();
        }
        s
    }

    #[test]
    fn starts_with_one_focused_empty_bar() {
        let s = SearchState::default();
        assert_eq!(s.bars.len(), 1);
        assert_eq!(s.newest(), FIRST_BAR);
        assert_eq!(s.focus_bar, Some(FIRST_BAR));
        assert_eq!(s.combined_query(), "");
        assert_eq!(s.texts(), [""]);
    }

    #[test]
    fn ids_are_stable_and_never_reused() {
        let mut s = state(&["a", "b", "c"]);
        let ids: Vec<u32> = s.bars.iter().map(|b| b.id).collect();
        assert_eq!(ids, [0, 1, 2]);
        assert_eq!(s.close(1), Some(0), "focus goes to the bar above");
        assert_eq!(s.bars.iter().map(|b| b.id).collect::<Vec<_>>(), [0, 2]);
        assert_eq!(s.texts(), ["a", "c"]);
        assert_eq!(s.add(), Some(3));
        assert_eq!(s.newest(), 3);
    }

    #[test]
    fn the_first_bar_cannot_be_closed() {
        let mut s = state(&["a", "b"]);
        assert_eq!(s.close(FIRST_BAR), None);
        assert_eq!(s.close(99), None);
        assert_eq!(s.bars.len(), 2);
    }

    #[test]
    fn at_most_eight_bars() {
        let mut s = SearchState::default();
        for _ in 1..MAX_SEARCH_BARS {
            assert!(s.add().is_some());
        }
        assert!(!s.can_add());
        assert_eq!(s.add(), None);
        assert_eq!(s.bars.len(), MAX_SEARCH_BARS);
    }

    #[test]
    fn errors_attach_to_one_bar() {
        let mut s = state(&["a", "\"b", "c"]);
        s.set_error(Some((1, "unterminated quote".to_owned())));
        let errors: Vec<_> = s.bars.iter().map(|b| b.error.is_some()).collect();
        assert_eq!(errors, [false, true, false]);
        s.set_error(Some((7, "gone".to_owned())));
        assert_eq!(
            s.bars[2].error.as_deref(),
            Some("gone"),
            "clamped to the last bar"
        );
        s.set_error(None);
        assert!(s.bars.iter().all(|b| b.error.is_none()));
    }

    #[test]
    fn combined_query_joins_active_bars_with_and() {
        assert_eq!(state(&[""]).combined_query(), "");
        assert_eq!(state(&["holiday draft"]).combined_query(), "holiday draft");
        assert_eq!(
            state(&["rust", " ", "path:src", "dm:thisweek"]).combined_query(),
            "rust AND path:src AND dm:thisweek"
        );
        assert_eq!(
            state(&["holiday draft", "ext:jpg"]).combined_query(),
            "(holiday draft) AND ext:jpg"
        );
        assert_eq!(state(&["", "x"]).combined_query(), "x");
    }
}

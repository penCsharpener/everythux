//! The search history dropdown: remembering searches, picking one and deleting one.

use egui::{Key, Modifiers};
use everythux_core::history::HistoryEntry;

use super::App;
use crate::search_bars::FIRST_BAR;

/// Seconds since the Unix epoch.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// State of the dropdown.
#[derive(Debug, Default)]
pub(crate) struct HistoryDropdown {
    pub(crate) open: bool,
    /// Index into the entries as listed (newest first).
    pub(crate) selected: usize,
}

impl App {
    /// The searches the dropdown lists, newest first.
    pub(crate) fn history_entries(&self) -> Vec<HistoryEntry> {
        self.history.newest_first().into_iter().cloned().collect()
    }

    /// Remembers the current search as used now (blank searches are skipped).
    pub(crate) fn record_history(&mut self) {
        if self.history.touch(&self.search.texts(), unix_now()) {
            self.backend.save_history(&self.history);
        }
    }

    /// Opens or closes the dropdown, which lists every remembered search.
    pub(crate) fn toggle_history(&mut self) {
        self.dropdown.open = !self.dropdown.open;
        self.dropdown.selected = 0;
        self.search.focus_bar = Some(FIRST_BAR);
    }

    /// Runs the remembered search `entry` again and moves it to the top of the history.
    pub(crate) fn pick_history(&mut self, entry: &HistoryEntry) {
        self.dropdown.open = false;
        self.search.set_texts(&entry.texts);
        self.history.touch(&entry.texts, unix_now());
        self.backend.save_history(&self.history);
        self.search.focus_bar = Some(FIRST_BAR);
        self.resubmit_search();
    }

    /// Forgets the remembered search `entry` (Shift+Delete on its line).
    pub(crate) fn delete_history(&mut self, entry: &HistoryEntry) {
        if self.history.remove(&entry.texts) {
            self.backend.save_history(&self.history);
        }
    }

    /// Keyboard of the open dropdown, read before any other shortcut: Up/Down move, Enter picks,
    /// Shift+Delete forgets the line, Escape closes. Alt+Down opens it from a search bar.
    pub(crate) fn history_keys(&mut self, ctx: &egui::Context) {
        let in_bar = self.focused_bar_in(ctx, false).is_some();
        if !self.dropdown.open {
            if in_bar && ctx.input_mut(|i| i.consume_key(Modifiers::ALT, Key::ArrowDown)) {
                self.toggle_history();
            }
            return;
        }
        let entries = self.history_entries();
        let last = entries.len().saturating_sub(1);
        self.dropdown.selected = self.dropdown.selected.min(last);
        let (up, down, enter, delete, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                // Shift+Delete, as in Windows drop-downs; Shift+Backspace is the Mac habit.
                i.consume_key(Modifiers::SHIFT, Key::Delete)
                    || i.consume_key(Modifiers::SHIFT, Key::Backspace),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            self.dropdown.open = false;
        } else if up {
            self.dropdown.selected = self.dropdown.selected.saturating_sub(1);
        } else if down {
            self.dropdown.selected = (self.dropdown.selected + 1).min(last);
        } else if delete {
            if let Some(entry) = entries.get(self.dropdown.selected) {
                self.delete_history(entry);
                self.dropdown.selected = self.dropdown.selected.min(last.saturating_sub(1));
            }
        } else if enter {
            match entries.get(self.dropdown.selected) {
                Some(entry) => self.pick_history(entry),
                None => self.dropdown.open = false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::tests::TestApp;

    #[test]
    fn using_a_search_remembers_it_and_reuse_moves_it_up() {
        let mut t = TestApp::new();
        t.app.set_search_text("report");
        t.app.record_history();
        t.app.set_search_text("notes");
        t.app.record_history();
        let order = |t: &TestApp| -> Vec<String> {
            t.app
                .history_entries()
                .iter()
                .map(everythux_core::history::HistoryEntry::label)
                .collect()
        };
        assert_eq!(order(&t).len(), 2);
        let report = t
            .app
            .history_entries()
            .into_iter()
            .find(|e| e.label() == "report");
        t.app.pick_history(&report.expect("remembered"));
        assert_eq!(order(&t)[0], "report");
        assert_eq!(t.app.search.texts(), ["report"]);
        let saved = t.fake.last_saved_history().expect("saved");
        assert_eq!(saved.entries.len(), 2);
    }

    #[test]
    fn deleting_forgets_the_search() {
        let mut t = TestApp::new();
        t.app.set_search_text("report");
        t.app.record_history();
        let entry = t.app.history_entries().remove(0);
        t.app.delete_history(&entry);
        assert_eq!(t.app.history_entries(), []);
        assert_eq!(t.fake.last_saved_history().expect("saved").entries.len(), 0);
    }
}

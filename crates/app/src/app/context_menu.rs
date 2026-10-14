//! The extra entries of the row context menu: "Open with", file actions installed by other
//! programs, copy-as-files and move-to-trash.
//!
//! The catalog of installed applications and actions is read once on a worker; filtering it for the
//! right-clicked selection is in-memory and runs when the menu opens.

use std::path::PathBuf;

use everythux_platform::actions::{MenuEntry, Target};

use super::App;
use crate::status::ToastKind;

/// Most selected rows considered when filtering entries for the menu.
const MAX_TARGETS: usize = 500;

/// Entries and targets of the menu that was opened last.
#[derive(Debug, Default)]
pub(crate) struct ContextExtras {
    /// "Open with" entries first, then file actions.
    pub(crate) entries: Vec<MenuEntry>,
    /// How many leading entries are "Open with" applications.
    pub(crate) open_with: usize,
    /// The selection the entries were filtered for.
    pub(crate) paths: Vec<PathBuf>,
}

impl ContextExtras {
    pub(crate) fn open_with(&self) -> &[MenuEntry] {
        &self.entries[..self.open_with]
    }

    pub(crate) fn actions(&self) -> &[MenuEntry] {
        &self.entries[self.open_with..]
    }
}

/// Drops hidden entries and moves the configured ones to the front, in the configured order.
fn arrange(mut entries: Vec<MenuEntry>, hidden: &[String], order: &[String]) -> Vec<MenuEntry> {
    entries.retain(|e| !hidden.contains(&e.id));
    // `sort_by_key` is stable: unlisted entries keep their natural order after the listed ones.
    entries.sort_by_key(|e| order.iter().position(|id| id == &e.id).unwrap_or(usize::MAX));
    entries
}

impl App {
    /// Rebuilds the extra entries for the current selection (called when a row is right-clicked).
    pub(crate) fn refresh_context_extras(&mut self) {
        let Some(results) = &self.results else {
            self.context_extras = ContextExtras::default();
            return;
        };
        let targets: Vec<Target> = self
            .selection
            .rows()
            .take(MAX_TARGETS)
            .filter_map(|row| {
                results.full_path(row).map(|path| Target {
                    path,
                    is_dir: results.is_dir(row),
                })
            })
            .collect();
        let menu = &self.config.ui.context_menu;
        let open_with = if menu.open_with {
            arrange(self.actions.open_with(&targets), &menu.hidden, &menu.order)
        } else {
            Vec::new()
        };
        let actions = if menu.service_menus {
            arrange(self.actions.actions_for(&targets), &menu.hidden, &menu.order)
        } else {
            Vec::new()
        };
        self.context_extras = ContextExtras {
            open_with: open_with.len(),
            entries: open_with.into_iter().chain(actions).collect(),
            paths: targets.into_iter().map(|t| t.path).collect(),
        };
        // The catalog is read at startup; installing a program later shows up after a refresh.
        if self.now - self.actions_loaded_at > ACTIONS_MAX_AGE {
            self.actions_loaded_at = self.now;
            self.backend.load_actions();
        }
    }

    /// "Hide" on a menu entry: remembered in the config until restored from View → Context menu.
    pub(crate) fn hide_context_entry(&mut self, index: usize) {
        let Some(entry) = self.context_extras.entries.get(index) else {
            return;
        };
        let (id, label) = (entry.id.clone(), entry.label.clone());
        let hidden = &mut self.config.ui.context_menu.hidden;
        if !hidden.contains(&id) {
            hidden.push(id);
        }
        self.config_changed();
        self.toast(
            format!("Hid \"{label}\" from the context menu (View → Context menu brings it back)"),
            ToastKind::Info,
        );
    }

    pub(crate) fn restore_hidden_context_entries(&mut self) {
        self.config.ui.context_menu.hidden.clear();
        self.config_changed();
    }

    pub(crate) fn toggle_context_open_with(&mut self) {
        let menu = &mut self.config.ui.context_menu;
        menu.open_with = !menu.open_with;
        self.config_changed();
    }

    pub(crate) fn toggle_context_service_menus(&mut self) {
        let menu = &mut self.config.ui.context_menu;
        menu.service_menus = !menu.service_menus;
        self.config_changed();
    }
}

/// Seconds after which opening the menu asks for the catalog again.
const ACTIONS_MAX_AGE: f64 = 60.0;

#[cfg(test)]
mod tests {
    use super::*;
    use everythux_platform::Dialect;
    use everythux_platform::actions::{Launcher, Source};

    fn entry(id: &str) -> MenuEntry {
        MenuEntry {
            id: id.to_owned(),
            label: id.to_owned(),
            submenu: None,
            source: Source::ServiceMenu,
            launcher: Launcher {
                dialect: Dialect::DesktopEntry,
                exec: "x".to_owned(),
                name: id.to_owned(),
                icon: None,
                desktop_file: None,
            },
        }
    }

    #[test]
    fn hidden_entries_disappear_and_ordered_ones_come_first() {
        let entries = vec![entry("a"), entry("b"), entry("c"), entry("d")];
        let out = arrange(entries, &["b".to_owned()], &["d".to_owned(), "c".to_owned()]);
        let ids: Vec<&str> = out.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["d", "c", "a"]);
    }
}

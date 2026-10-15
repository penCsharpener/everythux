//! Z-order bookkeeping for popups and windows, so `Escape` closes the topmost one.

use super::{App, PopupId};

impl App {
    /// Whether `id` is currently shown.
    pub(crate) fn is_popup_open(&self, id: PopupId) -> bool {
        let w = &self.windows;
        match id {
            PopupId::ConfirmReindex => w.confirm_reindex,
            PopupId::ConfirmDelete => w.confirm_delete.is_some(),
            PopupId::ExclusionPopup => w.exclusion_popup.is_some(),
            PopupId::ExclusionEditor => w.exclusion_editor.open,
            PopupId::Volumes => w.volumes.open,
            PopupId::Properties(n) => w.properties.iter().any(|p| p.id == n && p.open),
            PopupId::Shortcuts => w.shortcuts,
            PopupId::About => w.about,
            PopupId::PreviewWidth => self.preview.width_window,
            PopupId::Bookmarks => w.bookmarks.open,
        }
    }

    /// Brings `id` to the top of the stack.
    pub(crate) fn open_popup(&mut self, id: PopupId) {
        self.windows.order.retain(|p| *p != id);
        self.windows.order.push(id);
    }

    /// Closes `id`. Closing the exclusion popup commits it (see `finish_exclusion_popup`).
    pub(crate) fn close_popup(&mut self, id: PopupId) {
        match id {
            PopupId::ConfirmReindex => self.windows.confirm_reindex = false,
            PopupId::ConfirmDelete => self.windows.confirm_delete = None,
            PopupId::ExclusionPopup => self.finish_exclusion_popup(),
            PopupId::ExclusionEditor => self.windows.exclusion_editor.open = false,
            PopupId::Volumes => self.windows.volumes.open = false,
            PopupId::Properties(n) => self.windows.properties.retain(|p| p.id != n),
            PopupId::Shortcuts => self.windows.shortcuts = false,
            PopupId::About => self.windows.about = false,
            PopupId::PreviewWidth => self.preview.width_window = false,
            PopupId::Bookmarks => self.windows.bookmarks.open = false,
        }
        self.windows.order.retain(|p| *p != id);
    }

    /// Closes the topmost open popup; returns whether there was one.
    pub(crate) fn close_topmost_popup(&mut self) -> bool {
        while let Some(&id) = self.windows.order.last() {
            if self.is_popup_open(id) {
                self.close_popup(id);
                return true;
            }
            self.windows.order.pop();
        }
        false
    }

    /// Drops stack entries for windows the user closed with their own close button.
    pub(crate) fn sync_popups(&mut self) {
        let open: Vec<PopupId> = self
            .windows
            .order
            .iter()
            .copied()
            .filter(|id| self.is_popup_open(*id))
            .collect();
        self.windows.order = open;
        self.windows.properties.retain(|p| p.open);
    }
}

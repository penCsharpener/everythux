//! The built-in Properties window, used when the file manager can't show its own.

use crate::app::{App, PopupId};
use crate::backend::Compute;
use crate::model::{PropTarget, PropertiesInfo};

/// One open Properties window.
#[derive(Debug)]
pub(crate) struct PropertiesWindow {
    pub(crate) id: u64,
    pub(crate) target: PropTarget,
    /// Filled in by the compute worker; `None` while loading.
    pub(crate) info: Option<PropertiesInfo>,
    pub(crate) open: bool,
}

impl App {
    /// Opens the built-in window for `target`. Several windows can be open at once; asking for a
    /// target that already has one just raises it.
    pub(crate) fn open_properties_window(&mut self, target: PropTarget) {
        if let Some(existing) = self
            .windows
            .properties
            .iter()
            .find(|w| w.open && w.target.path == target.path)
        {
            let id = PopupId::Properties(existing.id);
            self.open_popup(id);
            return;
        }
        let id = self.next_request_id();
        let snapshot = self
            .results
            .as_ref()
            .map_or_else(|| self.snapshot.clone(), |r| r.snapshot.clone());
        self.windows.properties.push(PropertiesWindow {
            id,
            target: target.clone(),
            info: None,
            open: true,
        });
        self.open_popup(PopupId::Properties(id));
        self.backend.compute(Compute::Properties { id, snapshot, target });
    }

    pub(crate) fn on_properties(&mut self, id: u64, info: PropertiesInfo) {
        if let Some(window) = self.windows.properties.iter_mut().find(|w| w.id == id) {
            window.info = Some(info);
        }
    }
}

#[cfg(test)]
mod tests {
    use everythux_core::config::ColumnId;

    use crate::app::tests::TestApp;
    use crate::app::{CellEvent, PopupId};
    use egui::Modifiers;

    #[test]
    fn unsupported_properties_open_the_builtin_window() {
        let mut t = TestApp::new();
        t.fake.set_properties_unsupported(true);
        t.show_results(&[t.fixture.report]);
        let fx = t
            .app
            .on_cell_event(0, ColumnId::Size, CellEvent::DoubleClick(Modifiers::NONE));
        t.app.apply_effects(fx);
        assert!(
            t.app.windows.properties.is_empty(),
            "not before the platform answered"
        );
        t.pump();
        assert_eq!(t.app.windows.properties.len(), 1);
        let window = &t.app.windows.properties[0];
        assert_eq!(window.target.path, std::path::Path::new("/home/me/docs"));
        assert_eq!(t.app.windows.order.last(), Some(&PopupId::Properties(window.id)));
        t.pump();
        let info = t.app.windows.properties[0].info.as_ref().expect("info computed");
        assert_eq!((info.files, info.folders), (2, 0));
        assert!(info.meta.is_err(), "the fixture path doesn't exist on disk");
    }

    #[test]
    fn supported_properties_do_not_open_the_builtin_window() {
        let mut t = TestApp::new();
        t.show_results(&[t.fixture.report]);
        let fx = t
            .app
            .on_cell_event(0, ColumnId::Modified, CellEvent::DoubleClick(Modifiers::NONE));
        t.app.apply_effects(fx);
        t.pump();
        assert!(t.app.windows.properties.is_empty());
        assert_eq!(
            t.app.toasts.last().expect("toast").text,
            "Opened properties (FileManager1)"
        );
    }

    #[test]
    fn several_windows_can_be_open_but_not_duplicates() {
        let mut t = TestApp::new();
        let target = |p: &str| crate::model::PropTarget {
            path: p.into(),
            kind: everythux_core::model::EntryKind::Dir,
            size: 0,
            mtime_ns: 0,
        };
        t.app.open_properties_window(target("/a"));
        t.app.open_properties_window(target("/b"));
        t.app.open_properties_window(target("/a"));
        assert_eq!(t.app.windows.properties.len(), 2);
        assert!(t.app.close_topmost_popup());
        assert_eq!(t.app.windows.properties.len(), 1, "Escape closes the topmost one");
    }
}

//! Folding backend events into the app state.

use everythux_index::IndexEvent;
use everythux_platform::PlatformError;
use everythux_platform::file_manager::Method;

use super::{App, Banner};
use crate::backend::{BackendEvent, PlatformAction};
use crate::status::{Toast, ToastKind, ToastUndo};

/// How long a status-bar message stays visible, in seconds.
const TOAST_SECONDS: f64 = 6.0;
/// Messages with an "Undo" button stay a little longer.
const TOAST_UNDO_SECONDS: f64 = 10.0;

fn method_label(method: Method) -> &'static str {
    match method {
        Method::FileManager1 => "FileManager1",
        Method::Portal => "portal",
        Method::XdgOpen => "xdg-open",
    }
}

impl App {
    /// Drains the event channel. Called at the start of every frame.
    pub(crate) fn poll_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            self.handle_event(event);
        }
    }

    pub(crate) fn handle_event(&mut self, event: BackendEvent) {
        match event {
            BackendEvent::Index(ev) => self.on_index_event(ev),
            BackendEvent::Search(res) => self.on_search_results(res),
            BackendEvent::Preview(res) => self.on_preview_result(res),
            BackendEvent::Platform { action, result } => self.on_platform_outcome(action, result),
            BackendEvent::Renamed {
                from,
                new_name,
                undo,
                result,
            } => self.on_renamed(&from, &new_name, undo, result),
            BackendEvent::Devices(result) => self.on_devices(result),
            BackendEvent::Mounted { object_path, result } => self.on_mounted(&object_path, result),
            BackendEvent::ExclusionPreview(preview) => self.on_exclusion_preview(preview),
            BackendEvent::Properties { id, info } => self.on_properties(id, info),
            BackendEvent::RulesImported(result) => self.on_rules_imported(result),
            BackendEvent::RulesExported(result) => self.on_rules_exported(result),
            BackendEvent::ConfigSaveFailed(error) => {
                self.add_banner(format!("Could not save the configuration: {error}"));
            }
            BackendEvent::Fatal(message) => self.add_banner(message),
        }
    }

    fn on_index_event(&mut self, event: IndexEvent) {
        self.index.on_event(&event);
        match event {
            IndexEvent::SnapshotReady(snapshot) => {
                self.snapshot = snapshot.clone();
                self.backend.set_snapshot(snapshot);
            }
            IndexEvent::VolumesChanged(volumes) => self.volumes = volumes,
            IndexEvent::Failed { error, .. } => {
                tracing::error!(%error, "index job failed");
            }
            _ => {}
        }
    }

    fn on_platform_outcome(&mut self, action: PlatformAction, result: Result<Method, PlatformError>) {
        match (action, result) {
            (PlatformAction::ShowProperties(target), Err(error)) => {
                tracing::info!(%error, path = %target.path.display(), "file manager has no properties; using built-in window");
                self.open_properties_window(target);
            }
            (action, Err(error)) => {
                tracing::warn!(%error, action = %action.describe(), "desktop action failed");
                self.toast(format!("{} failed: {error}", action.describe()), ToastKind::Error);
            }
            (PlatformAction::OpenDefault(path), Ok(method)) => {
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                self.toast(
                    format!("Opened {name} ({})", method_label(method)),
                    ToastKind::Info,
                );
            }
            (PlatformAction::ShowItem(_), Ok(method)) => {
                self.toast(
                    format!("Opened in file manager ({})", method_label(method)),
                    ToastKind::Info,
                );
            }
            // Renames report through `BackendEvent::Renamed`.
            (PlatformAction::Rename { .. }, Ok(_)) => {}
            (PlatformAction::ShowProperties(_), Ok(method)) => {
                self.toast(
                    format!("Opened properties ({})", method_label(method)),
                    ToastKind::Info,
                );
            }
        }
    }

    /// Adds a status-bar message.
    pub(crate) fn toast(&mut self, text: impl Into<String>, kind: ToastKind) {
        self.push_toast(text.into(), kind, None);
    }

    /// Adds a status-bar message with an Undo button.
    pub(crate) fn toast_with_undo(&mut self, text: impl Into<String>, undo: ToastUndo) {
        self.push_toast(text.into(), ToastKind::Info, Some(undo));
    }

    fn push_toast(&mut self, text: String, kind: ToastKind, undo: Option<ToastUndo>) {
        let seconds = if undo.is_some() {
            TOAST_UNDO_SECONDS
        } else {
            TOAST_SECONDS
        };
        self.toasts.clear();
        self.toasts.push(Toast {
            text,
            kind,
            expires: self.now + seconds,
            undo,
        });
        self.ctx
            .request_repaint_after(std::time::Duration::from_secs_f64(seconds + 0.05));
    }

    /// Shows a warning banner at the top (deduplicated).
    pub(crate) fn add_banner(&mut self, text: String) {
        tracing::warn!(%text, "banner");
        if !self.banners.iter().any(|b| b.text == text) {
            self.banners.push(Banner { text });
        }
    }

    /// Drops expired messages.
    pub(crate) fn expire_toasts(&mut self) {
        let now = self.now;
        self.toasts.retain(|t| t.expires > now);
    }
}

#[cfg(test)]
mod tests {
    use everythux_index::{JobId, JobStats};

    use super::*;
    use crate::app::tests::TestApp;
    use crate::backend::fake::Call;

    #[test]
    fn snapshot_ready_is_forwarded_to_the_search_service() {
        let mut t = TestApp::new();
        t.fake.clear();
        t.fake.send(BackendEvent::Index(IndexEvent::SnapshotReady(
            t.fixture.snapshot.clone(),
        )));
        t.pump();
        assert!(std::sync::Arc::ptr_eq(&t.app.snapshot, &t.fixture.snapshot));
        assert!(t.fake.calls().contains(&Call::SetSnapshot));
    }

    #[test]
    fn index_events_drive_the_status() {
        let mut t = TestApp::new();
        t.fake.send(BackendEvent::Index(IndexEvent::Started {
            job: JobId(1),
            description: "Indexing /home".into(),
        }));
        t.pump();
        assert!(t.app.index.is_running());
        t.fake.send(BackendEvent::Index(IndexEvent::Finished {
            job: JobId(1),
            stats: JobStats::default(),
        }));
        t.pump();
        assert!(!t.app.index.is_running());
    }

    #[test]
    fn platform_success_shows_a_message() {
        let mut t = TestApp::new();
        t.app.handle_event(BackendEvent::Platform {
            action: PlatformAction::ShowItem("/home/me/docs/report.pdf".into()),
            result: Ok(Method::FileManager1),
        });
        assert_eq!(t.app.toasts[0].text, "Opened in file manager (FileManager1)");
    }

    #[test]
    fn platform_failure_shows_an_error() {
        let mut t = TestApp::new();
        t.app.handle_event(BackendEvent::Platform {
            action: PlatformAction::OpenDefault("/x/y.txt".into()),
            result: Err(PlatformError::Unsupported("nothing")),
        });
        assert_eq!(t.app.toasts[0].kind, ToastKind::Error);
        assert!(
            t.app.toasts[0].text.contains("failed"),
            "{}",
            t.app.toasts[0].text
        );
    }

    #[test]
    fn fatal_and_save_errors_become_banners_once() {
        let mut t = TestApp::new();
        t.app.handle_event(BackendEvent::Fatal("db broken".into()));
        t.app.handle_event(BackendEvent::Fatal("db broken".into()));
        t.app
            .handle_event(BackendEvent::ConfigSaveFailed("disk full".into()));
        assert_eq!(t.app.banners.len(), 2);
    }

    #[test]
    fn toasts_expire() {
        let mut t = TestApp::new();
        t.app.now = 100.0;
        t.app.toast("hello", ToastKind::Info);
        t.app.now = 100.0 + TOAST_SECONDS - 1.0;
        t.app.expire_toasts();
        assert_eq!(t.app.toasts.len(), 1);
        t.app.now = 100.0 + TOAST_SECONDS + 1.0;
        t.app.expire_toasts();
        assert!(t.app.toasts.is_empty(), "{:?}", t.app.toasts);
    }
}

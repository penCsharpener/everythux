//! Preview pane state (Phase 8): selection debounce, request tracking and texture ownership.

use std::path::PathBuf;
use std::time::Duration;

use everythux_preview::{ImageData, PreviewContent, PreviewLimits, PreviewResult, TEXT_MORE_BYTES};

use crate::app::App;
use crate::text_doc::TextDoc;

/// Selection must rest this long before a preview is requested.
pub(crate) const PREVIEW_DEBOUNCE: f64 = 0.1;

/// A finished preview with its uploaded textures. Dropping it frees the textures.
pub(crate) struct PreviewShown {
    pub(crate) path: PathBuf,
    pub(crate) content: PreviewContent,
    /// One handle per image of `content`, in the order of [`images_of`].
    pub(crate) textures: Vec<egui::TextureHandle>,
    /// For text previews: the text, moved out of `content` (whose `text` is left empty) and
    /// indexed for rendering. Grows with "Show more".
    pub(crate) text: Option<TextDoc>,
}

impl std::fmt::Debug for PreviewShown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewShown")
            .field("path", &self.path)
            .field("textures", &self.textures.len())
            .finish_non_exhaustive()
    }
}

/// Preview pane state.
#[derive(Debug, Default)]
pub(crate) struct PreviewState {
    /// The entry the selection currently points at.
    pub(crate) wanted: Option<PathBuf>,
    /// When `wanted` last changed.
    pub(crate) wanted_since: f64,
    /// The request in flight or already shown: path and request id.
    pub(crate) requested: Option<(PathBuf, u64)>,
    pub(crate) shown: Option<PreviewShown>,
    /// The pane was resized and the new width is not saved yet.
    pub(crate) width_dirty: bool,
    /// The "Custom…" pane width window is open.
    pub(crate) width_window: bool,
    /// Outer rect of the pane in the last frame.
    pub(crate) pane_rect: Option<egui::Rect>,
    /// Rect covered by the pane's content in the last frame (wider than the pane if it overflowed).
    pub(crate) content_rect: Option<egui::Rect>,
    /// Id of the "Show more" text request in flight.
    pub(crate) more_request: Option<u64>,
    /// Why the last "Show more" failed.
    pub(crate) more_error: Option<String>,
}

impl PreviewState {
    /// Forgets everything (pane hidden, selection cleared).
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The images inside a preview, in a fixed order.
pub(crate) fn images_of(content: &PreviewContent) -> Vec<&ImageData> {
    match content {
        PreviewContent::Image { image, .. } => vec![image],
        PreviewContent::Pdf { page, .. } => vec![page],
        PreviewContent::Audio { cover, .. } => cover.iter().collect(),
        PreviewContent::Video { thumbnail, .. } => thumbnail.iter().collect(),
        _ => Vec::new(),
    }
}

fn to_color_image(image: &ImageData) -> Option<egui::ColorImage> {
    let (w, h) = (image.width as usize, image.height as usize);
    (w > 0 && h > 0 && image.rgba.len() == w * h * 4)
        .then(|| egui::ColorImage::from_rgba_unmultiplied([w, h], &image.rgba))
}

impl App {
    /// The preview limits derived from the config.
    pub(crate) fn preview_limits(&self) -> PreviewLimits {
        PreviewLimits {
            max_file_size: self.config.ui.preview_pane.max_file_size,
            ..PreviewLimits::default()
        }
    }

    /// Called once per frame: debounces selection changes and requests the preview.
    pub(crate) fn drive_preview(&mut self, ctx: &egui::Context) {
        if !self.config.ui.preview_pane.enabled {
            return;
        }
        let wanted = self
            .selection
            .cursor()
            .filter(|_| !self.selection.is_empty())
            .and_then(|row| self.results.as_ref().and_then(|r| r.full_path(row)));
        if wanted != self.preview.wanted {
            self.preview.wanted_since = self.now;
            if wanted.is_none() {
                self.preview.shown = None;
                self.preview.requested = None;
            }
            self.preview.wanted = wanted;
        }
        let Some(path) = self.preview.wanted.clone() else {
            return;
        };
        if self.preview.requested.as_ref().is_some_and(|(p, _)| *p == path) {
            return;
        }
        let idle = self.now - self.preview.wanted_since;
        if idle >= PREVIEW_DEBOUNCE {
            let id = self.backend.request_preview(path.clone(), self.preview_limits());
            self.preview.requested = Some((path, id));
        } else {
            ctx.request_repaint_after(Duration::from_secs_f64(PREVIEW_DEBOUNCE - idle + 0.005));
        }
    }

    /// Uploads the textures of a finished preview (once) and replaces the shown one.
    pub(crate) fn on_preview_result(&mut self, result: PreviewResult) {
        if self.preview.more_request.is_some_and(|id| id == result.request) {
            self.preview.more_request = None;
            self.on_more_text(result);
            return;
        }
        let current = self.preview.requested.as_ref().map(|(_, id)| *id);
        if current != Some(result.request) {
            return;
        }
        let mut content = result.content;
        let text = match &mut content {
            PreviewContent::Text {
                text,
                encoding,
                total_bytes,
                end_offset,
                ..
            } => Some(TextDoc::new(
                std::mem::take(text),
                *encoding,
                *total_bytes,
                *end_offset,
            )),
            _ => None,
        };
        self.preview.more_request = None;
        self.preview.more_error = None;
        let textures = images_of(&content)
            .into_iter()
            .enumerate()
            .filter_map(|(i, image)| {
                let color = to_color_image(image)?;
                Some(self.ctx.load_texture(
                    format!("preview-{}-{i}", result.request),
                    color,
                    egui::TextureOptions::LINEAR,
                ))
            })
            .collect();
        self.preview.shown = Some(PreviewShown {
            path: result.path,
            content,
            textures,
            text,
        });
    }

    /// Appends a "Show more" chunk to the shown text.
    fn on_more_text(&mut self, result: PreviewResult) {
        let Some(doc) = self
            .preview
            .shown
            .as_mut()
            .filter(|s| s.path == result.path)
            .and_then(|s| s.text.as_mut())
        else {
            return;
        };
        match result.content {
            PreviewContent::Text {
                text,
                total_bytes,
                end_offset,
                ..
            } => doc.append(&text, total_bytes, end_offset),
            PreviewContent::Unavailable { hint } => self.preview.more_error = Some(hint),
            other => tracing::debug!(?other, "unexpected answer to a text range request"),
        }
    }

    /// "Show more": asks the preview worker for the next chunk of the shown text.
    pub(crate) fn request_more_text(&mut self) {
        if self.preview.more_request.is_some() {
            return;
        }
        let Some((path, doc)) = self
            .preview
            .shown
            .as_ref()
            .and_then(|s| Some((s.path.clone(), s.text.as_ref()?)))
        else {
            return;
        };
        if !doc.more_available() {
            return;
        }
        let id = self
            .backend
            .request_text_range(path, doc.end_offset, TEXT_MORE_BYTES, doc.encoding);
        self.preview.more_request = Some(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::TestApp;
    use crate::backend::BackendEvent;
    use crate::backend::fake::Call;

    fn rgba(w: u32, h: u32) -> ImageData {
        ImageData {
            width: w,
            height: h,
            rgba: vec![255; (w * h * 4) as usize],
        }
    }

    fn preview_calls(t: &TestApp) -> Vec<PathBuf> {
        t.fake
            .calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::RequestPreview(p) => Some(p),
                _ => None,
            })
            .collect()
    }

    fn enable(t: &mut TestApp) {
        t.app.config.ui.preview_pane.enabled = true;
        t.show_results(&[t.fixture.report, t.fixture.notes]);
    }

    #[test]
    fn selection_changes_are_debounced() {
        let mut t = TestApp::new();
        enable(&mut t);
        let ctx = t.app.ctx.clone();
        t.app.selection.select_only(0);
        t.app.now = 10.0;
        t.app.drive_preview(&ctx);
        assert!(preview_calls(&t).is_empty(), "too early");
        t.app.selection.select_only(1);
        t.app.now = 10.05;
        t.app.drive_preview(&ctx);
        assert!(
            preview_calls(&t).is_empty(),
            "selection moved again; timer restarts"
        );
        t.app.now = 10.2;
        t.app.drive_preview(&ctx);
        assert_eq!(preview_calls(&t), [PathBuf::from("/home/me/docs/notes.txt")]);
        t.app.now = 10.5;
        t.app.drive_preview(&ctx);
        assert_eq!(preview_calls(&t).len(), 1, "no repeat request for the same path");
    }

    #[test]
    fn nothing_is_requested_while_the_pane_is_hidden() {
        let mut t = TestApp::new();
        t.show_results(&[t.fixture.report]);
        t.app.selection.select_only(0);
        t.app.now = 50.0;
        let ctx = t.app.ctx.clone();
        t.app.drive_preview(&ctx);
        t.app.now = 51.0;
        t.app.drive_preview(&ctx);
        assert!(preview_calls(&t).is_empty(), "{:?}", preview_calls(&t));
    }

    #[test]
    fn results_for_old_requests_are_dropped() {
        let mut t = TestApp::new();
        enable(&mut t);
        let ctx = t.app.ctx.clone();
        t.app.selection.select_only(0);
        t.app.now = 1.0;
        t.app.drive_preview(&ctx);
        t.app.now = 2.0;
        t.app.drive_preview(&ctx);
        let (_, id) = t.app.preview.requested.clone().expect("requested");
        t.fake.send(BackendEvent::Preview(PreviewResult {
            request: id + 7,
            path: "/x".into(),
            content: PreviewContent::Unavailable { hint: "stale".into() },
        }));
        t.pump();
        assert!(t.app.preview.shown.is_none());
        t.fake.send(BackendEvent::Preview(PreviewResult {
            request: id,
            path: "/home/me/docs/report.pdf".into(),
            content: PreviewContent::Pdf {
                page: rgba(4, 4),
                pages: Some(3),
            },
        }));
        t.pump();
        let shown = t.app.preview.shown.as_ref().expect("shown");
        assert_eq!(shown.textures.len(), 1, "texture uploaded once");
    }

    #[test]
    fn malformed_images_do_not_panic() {
        let bad = ImageData {
            width: 10,
            height: 10,
            rgba: vec![0; 3],
        };
        assert!(to_color_image(&bad).is_none());
        assert!(to_color_image(&rgba(0, 0)).is_none());
        assert!(to_color_image(&rgba(2, 2)).is_some());
    }

    #[test]
    fn clearing_the_selection_clears_the_preview() {
        let mut t = TestApp::new();
        enable(&mut t);
        let ctx = t.app.ctx.clone();
        t.app.selection.select_only(0);
        t.app.now = 1.0;
        t.app.drive_preview(&ctx);
        t.app.now = 2.0;
        t.app.drive_preview(&ctx);
        t.app.selection.clear();
        t.app.drive_preview(&ctx);
        assert!(t.app.preview.requested.is_none());
        assert!(t.app.preview.wanted.is_none());
    }

    #[test]
    fn images_are_collected_per_content_kind() {
        let content = PreviewContent::Audio {
            meta: everythux_preview::MediaMeta::default(),
            cover: Some(rgba(1, 1)),
        };
        assert_eq!(images_of(&content).len(), 1);
        let text = PreviewContent::text("");
        assert_eq!(images_of(&text).len(), 0, "text previews carry no images");
    }
}

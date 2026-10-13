//! Preview providers for the optional preview pane (ROADMAP Phase 8).
//!
//! UI-toolkit agnostic: produces RGBA pixels / text / metadata; the app turns them into textures.
//! [`thumbnails::find_cached`] looks up the freedesktop thumbnail cache.
//! All work happens on [`PreviewService`]'s worker thread with an LRU cache; a new request
//! cancels the previous one.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::time::Duration;

#[cfg(feature = "audio")]
mod audio;
mod cache;
mod cancel;
mod env;
mod error;
#[cfg(feature = "image")]
mod images;
mod kind;
#[cfg(feature = "pdf")]
mod pdf;
mod render;
mod service;
mod textfile;
pub mod thumbnails;
#[cfg(any(feature = "pdf", feature = "video"))]
mod tools;
#[cfg(feature = "video")]
mod video;

pub use service::PreviewService;

/// Decoded RGBA8 image, already downscaled to fit the requested box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageData {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, RGBA, row-major.
    pub rgba: Vec<u8>,
}

/// Audio/video metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaMeta {
    /// Title tag.
    pub title: Option<String>,
    /// Artist tag.
    pub artist: Option<String>,
    /// Album tag.
    pub album: Option<String>,
    /// Duration.
    pub duration: Option<Duration>,
    /// Other key/value facts (codec, bitrate, resolution, sample rate, …).
    pub extra: Vec<(String, String)>,
}

/// What to show in the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PreviewContent {
    /// Picture (images, or a thumbnail from the freedesktop cache).
    Image {
        /// Pixels.
        image: ImageData,
        /// Original dimensions, if known.
        original_size: Option<(u32, u32)>,
    },
    /// Text / source code (first 64 KiB, lossily decoded).
    Text {
        /// Content.
        text: String,
        /// Whether the file was longer.
        truncated: bool,
    },
    /// PDF first page.
    Pdf {
        /// Rendered page.
        page: ImageData,
        /// Page count, if known.
        pages: Option<u32>,
    },
    /// Audio file.
    Audio {
        /// Tags and properties.
        meta: MediaMeta,
        /// Embedded cover art.
        cover: Option<ImageData>,
    },
    /// Video file.
    Video {
        /// Properties.
        meta: MediaMeta,
        /// Frame thumbnail.
        thumbnail: Option<ImageData>,
    },
    /// Folder: basic facts (computed by the caller is fine; provider returns children count).
    Folder {
        /// Number of direct children readable on disk.
        children: Option<u64>,
    },
    /// No visual preview; `hint` explains why or what to install (e.g. "Install poppler-utils for PDF previews").
    Unavailable {
        /// Explanation.
        hint: String,
    },
}

/// One finished preview.
#[derive(Debug, Clone)]
pub struct PreviewResult {
    /// Id returned by [`PreviewService::request`].
    pub request: u64,
    /// File previewed.
    pub path: PathBuf,
    /// Content.
    pub content: PreviewContent,
}

/// Limits for previews.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewLimits {
    /// Files larger than this get `Unavailable` (except media metadata, which reads headers only).
    pub max_file_size: u64,
    /// Images are downscaled to fit `max_dim × max_dim`.
    pub max_dim: u32,
}

impl Default for PreviewLimits {
    fn default() -> Self {
        Self {
            max_file_size: 512 * 1024 * 1024,
            max_dim: 768,
        }
    }
}

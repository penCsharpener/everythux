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
mod encoding;
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

pub use encoding::TextEncoding;
pub use service::PreviewService;
pub use textfile::more_text_available;

/// Bytes of a text file shown at first.
pub const TEXT_HEAD_BYTES: u64 = 64 * 1024;
/// Bytes one "Show more" adds ([`PreviewService::request_range`]).
pub const TEXT_MORE_BYTES: u64 = 256 * 1024;
/// Text previews never read past this many bytes of a file.
pub const TEXT_MAX_BYTES: u64 = 8 * 1024 * 1024;

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
    /// Text / source code: the first [`TEXT_HEAD_BYTES`] of the file, or the chunk asked for
    /// with [`PreviewService::request_range`], decoded to UTF-8.
    Text {
        /// Content. Invalid sequences are U+FFFD, NUL bytes the visible U+2400.
        text: String,
        /// Whether the file goes on after `end_offset`.
        truncated: bool,
        /// Encoding the file was decoded from.
        encoding: TextEncoding,
        /// Size of the file in bytes.
        total_bytes: u64,
        /// Byte offset in the file just after `text`: where the next chunk starts.
        end_offset: u64,
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

impl PreviewContent {
    /// A complete UTF-8 text preview of `text` (the whole file).
    pub fn text(text: impl Into<String>) -> Self {
        let text = text.into();
        let len = text.len() as u64;
        Self::Text {
            text,
            truncated: false,
            encoding: TextEncoding::Utf8,
            total_bytes: len,
            end_offset: len,
        }
    }
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

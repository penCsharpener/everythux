//! Internal error type of the providers. It never crosses the public API: the service turns every
//! error into [`crate::PreviewContent::Unavailable`] (or drops the result if it was cancelled).

use std::io;

#[derive(Debug, thiserror::Error)]
pub(crate) enum PreviewError {
    /// A newer request superseded this one.
    #[error("preview cancelled")]
    Cancelled,
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// The file is bigger than `PreviewLimits::max_file_size`.
    #[error("file is larger than the preview limit")]
    TooLarge { limit: u64 },
    /// The bytes could not be decoded or parsed.
    #[cfg(feature = "image")]
    #[error("cannot read this file: {0}")]
    Decode(String),
    /// An external helper is not installed.
    #[cfg(any(feature = "pdf", feature = "video"))]
    #[error("{tool} is not installed")]
    ToolMissing { tool: &'static str, hint: &'static str },
    #[cfg(any(feature = "pdf", feature = "video"))]
    #[error("{tool} timed out")]
    Timeout { tool: &'static str },
    #[cfg(any(feature = "pdf", feature = "video"))]
    #[error("{tool} failed")]
    ToolFailed { tool: &'static str },
    /// The provider's cargo feature is compiled out.
    #[cfg(any(
        not(feature = "image"),
        not(feature = "audio"),
        not(feature = "pdf"),
        not(feature = "video")
    ))]
    #[error("built without {0} support")]
    FeatureDisabled(&'static str),
    /// Not text (binary content without a preview).
    #[error("binary file")]
    NotText,
    /// A file with a text extension or name whose content is binary.
    #[error("binary file")]
    Binary,
    /// A text range past [`crate::TEXT_MAX_BYTES`] was requested.
    #[error("text previews stop at 8 MiB")]
    TextCap,
}

impl PreviewError {
    pub(crate) fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
            || matches!(self, Self::Io(e) if e.kind() == io::ErrorKind::Interrupted)
    }

    /// Text for the `Unavailable` pane.
    pub(crate) fn hint(&self) -> String {
        match self {
            Self::TooLarge { limit } => {
                format!("File is larger than the preview limit ({})", human_bytes(*limit))
            }
            #[cfg(any(feature = "pdf", feature = "video"))]
            Self::ToolMissing { hint, .. } => (*hint).to_owned(),
            #[cfg(any(feature = "pdf", feature = "video"))]
            Self::Timeout { tool } => format!("Preview timed out ({tool})"),
            Self::NotText => "No preview available for this file type".to_owned(),
            Self::Binary => "Binary file".to_owned(),
            Self::TextCap => format!("Text previews stop at {}", human_bytes(crate::TEXT_MAX_BYTES)),
            other => other.to_string(),
        }
    }
}

/// `1536` -> `"1.5 KiB"`.
pub(crate) fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_scales() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(512 * 1024 * 1024), "512.0 MiB");
    }

    #[test]
    fn interrupted_io_counts_as_cancelled() {
        let err = PreviewError::from(io::Error::new(io::ErrorKind::Interrupted, "x"));
        assert!(err.is_cancelled());
        assert!(!PreviewError::NotText.is_cancelled());
    }
}

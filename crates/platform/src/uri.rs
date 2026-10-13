//! `file://` URIs.

use std::path::Path;

/// Percent-encodes an absolute path (raw bytes, so non-UTF-8 names survive) as a `file://` URI.
pub fn file_uri(path: &Path) -> String {
    let _ = path;
    todo!()
}

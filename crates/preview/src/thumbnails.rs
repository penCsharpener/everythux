//! freedesktop thumbnail cache (`~/.cache/thumbnails/{normal,large,x-large,xx-large}/<md5(uri)>.png`).

use std::path::{Path, PathBuf};

/// Largest existing cached thumbnail for `path` that is not older than the file, if any.
pub fn find_cached(path: &Path) -> Option<PathBuf> {
    let _ = path;
    todo!()
}

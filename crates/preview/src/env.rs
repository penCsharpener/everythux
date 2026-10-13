//! Where providers find the outside world: helper programs and the thumbnail cache.
//! Kept in one value so tests can point it at fixtures instead of the real system.

use std::path::{Path, PathBuf};

use crate::thumbnails;
#[cfg(any(feature = "pdf", feature = "video"))]
use crate::tools::Tools;

#[derive(Debug)]
pub(crate) struct Env {
    #[cfg(any(feature = "pdf", feature = "video"))]
    pub(crate) tools: Tools,
    #[cfg_attr(
        not(feature = "image"),
        expect(
            dead_code,
            reason = "cached thumbnails are only decoded with the image feature"
        )
    )]
    /// Directory holding `normal/`, `large/`, … (see [`thumbnails`]).
    pub(crate) thumbnail_dir: Option<PathBuf>,
}

impl Env {
    /// `$PATH` and the XDG thumbnail cache.
    pub(crate) fn system() -> Self {
        Self {
            #[cfg(any(feature = "pdf", feature = "video"))]
            tools: Tools::from_env(),
            thumbnail_dir: thumbnails::cache_dir(),
        }
    }

    #[cfg_attr(
        not(feature = "image"),
        expect(
            dead_code,
            reason = "cached thumbnails are only decoded with the image feature"
        )
    )]
    /// Fresh thumbnail for `path` in the freedesktop cache, if any.
    pub(crate) fn cached_thumbnail(&self, path: &Path) -> Option<PathBuf> {
        thumbnails::find_cached_in(path, self.thumbnail_dir.as_deref()?)
    }
}

//! freedesktop thumbnail cache (`~/.cache/thumbnails/{normal,large,x-large,xx-large}/<md5(uri)>.png`).
//!
//! The file manager already generated these for most files it has shown, so reading them is the
//! cheapest preview there is.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_encode};

/// Cache sub-directories, largest first.
const SIZE_DIRS: [&str; 4] = ["xx-large", "x-large", "large", "normal"];

/// Everything except the RFC 3986 unreserved characters and `/` is escaped.
const URI_ESCAPE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

/// Largest existing cached thumbnail for `path` that is not older than the file, if any.
///
/// Looks in `$XDG_CACHE_HOME/thumbnails` (default `~/.cache/thumbnails`). A relative `path` is made
/// absolute against the current directory first, because the cache is keyed by absolute URI.
pub fn find_cached(path: &Path) -> Option<PathBuf> {
    find_cached_in(path, &cache_dir()?)
}

/// Like [`find_cached`] but looks in an explicit thumbnail directory (the one containing
/// `normal/`, `large/`, …). Useful for tests and for callers that resolve the XDG location themselves.
pub fn find_cached_in(path: &Path, thumbnail_dir: &Path) -> Option<PathBuf> {
    let absolute = std::path::absolute(path).ok()?;
    let file_mtime = std::fs::metadata(&absolute).ok()?.modified().ok()?;
    let name = format!("{}.png", thumbnail_name(&absolute));
    SIZE_DIRS.iter().find_map(|size| {
        let candidate = thumbnail_dir.join(size).join(&name);
        let thumb_mtime = std::fs::metadata(&candidate).ok()?.modified().ok()?;
        (thumb_mtime >= file_mtime).then_some(candidate)
    })
}

/// `$XDG_CACHE_HOME/thumbnails`, or `$HOME/.cache/thumbnails`.
pub fn cache_dir() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_CACHE_HOME");
    let home = std::env::var_os("HOME");
    cache_dir_from(xdg.as_deref(), home.as_deref())
}

fn cache_dir_from(xdg_cache_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    // The XDG spec says relative values must be ignored.
    let base = match xdg_cache_home.map(Path::new) {
        Some(dir) if dir.is_absolute() => dir.to_path_buf(),
        _ => Path::new(home.filter(|h| !h.is_empty())?).join(".cache"),
    };
    Some(base.join("thumbnails"))
}

/// `file://` URI of `path`: raw path bytes, percent-encoded (no lossy UTF-8 round trip).
pub(crate) fn file_uri(path: &Path) -> String {
    format!(
        "file://{}",
        percent_encode(path.as_os_str().as_bytes(), URI_ESCAPE)
    )
}

/// Lowercase hex MD5 of the file URI, which names the thumbnail.
pub(crate) fn thumbnail_name(path: &Path) -> String {
    let digest = Md5::digest(file_uri(path).as_bytes());
    digest.iter().fold(String::with_capacity(32), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs::{self, File};
    use std::time::{Duration, SystemTime};

    use super::*;

    fn set_mtime(path: &Path, time: SystemTime) {
        File::options()
            .write(true)
            .open(path)
            .expect("open")
            .set_modified(time)
            .expect("set mtime");
    }

    #[test]
    fn uri_encodes_raw_bytes_and_keeps_unreserved() {
        assert_eq!(
            file_uri(Path::new("/home/a/b-c_d.e~f")),
            "file:///home/a/b-c_d.e~f"
        );
        assert_eq!(file_uri(Path::new("/a b/ü#?%")), "file:///a%20b/%C3%BC%23%3F%25");
        let raw = Path::new(OsStr::from_bytes(b"/tmp/bad\xffname"));
        assert_eq!(file_uri(raw), "file:///tmp/bad%FFname");
    }

    #[test]
    fn name_is_md5_of_uri() {
        // Cross-checked with `printf 'file:///home/jens/photo/me.jpg' | md5sum`.
        assert_eq!(
            thumbnail_name(Path::new("/home/jens/photo/me.jpg")),
            "6a6e2356333b14814bde41eb5adb7653"
        );
    }

    #[test]
    fn cache_dir_follows_xdg_rules() {
        let os = |s: &str| OsString::from(s);
        assert_eq!(
            cache_dir_from(Some(&os("/x/cache")), Some(&os("/home/u"))),
            Some(PathBuf::from("/x/cache/thumbnails"))
        );
        assert_eq!(
            cache_dir_from(Some(&os("relative")), Some(&os("/home/u"))),
            Some(PathBuf::from("/home/u/.cache/thumbnails"))
        );
        assert_eq!(
            cache_dir_from(None, Some(&os("/home/u"))),
            Some(PathBuf::from("/home/u/.cache/thumbnails"))
        );
        assert_eq!(cache_dir_from(None, None), None);
    }

    #[test]
    fn picks_largest_fresh_thumbnail() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("movie file.mkv");
        fs::write(&file, b"x").expect("write");
        let cache = tmp.path().join("thumbs");
        let name = format!("{}.png", thumbnail_name(&file));
        for size in ["normal", "large"] {
            fs::create_dir_all(cache.join(size)).expect("mkdir");
            fs::write(cache.join(size).join(&name), b"png").expect("write thumb");
        }
        assert_eq!(
            find_cached_in(&file, &cache),
            Some(cache.join("large").join(&name))
        );

        fs::create_dir_all(cache.join("xx-large")).expect("mkdir");
        fs::write(cache.join("xx-large").join(&name), b"png").expect("write thumb");
        assert_eq!(
            find_cached_in(&file, &cache),
            Some(cache.join("xx-large").join(&name))
        );
    }

    #[test]
    fn stale_thumbnails_are_ignored_and_older_sizes_still_count() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("a.mp4");
        fs::write(&file, b"x").expect("write");
        let cache = tmp.path().join("thumbs");
        let name = format!("{}.png", thumbnail_name(&file));
        let now = SystemTime::now();
        set_mtime(&file, now);
        for (size, age) in [("large", 100u64), ("normal", 0)] {
            fs::create_dir_all(cache.join(size)).expect("mkdir");
            let thumb = cache.join(size).join(&name);
            fs::write(&thumb, b"png").expect("write thumb");
            // `large` is 100 s older than the file (stale), `normal` has the same mtime (fresh).
            set_mtime(&thumb, now - Duration::from_secs(age));
        }
        assert_eq!(
            find_cached_in(&file, &cache),
            Some(cache.join("normal").join(&name))
        );

        set_mtime(&cache.join("normal").join(&name), now - Duration::from_secs(1));
        assert_eq!(
            find_cached_in(&file, &cache),
            None,
            "all thumbnails older than the file"
        );
    }

    #[test]
    fn missing_file_or_cache_gives_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(find_cached_in(&tmp.path().join("nope"), tmp.path()), None);
        let file = tmp.path().join("f");
        fs::write(&file, b"x").expect("write");
        assert_eq!(find_cached_in(&file, &tmp.path().join("no-cache")), None);
    }
}

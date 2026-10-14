//! Moving files to the trash, following the freedesktop.org Trash specification 1.0.
//!
//! Files on the same filesystem as the home trash (`$XDG_DATA_HOME/Trash`) go there; files on other
//! filesystems go to a trash directory at the root of that filesystem (`$topdir/.Trash/$uid` when
//! the administrator set it up, else `$topdir/.Trash-$uid`). Nothing is ever copied: if no trash
//! directory on the file's own filesystem is usable the operation fails instead of moving data
//! across devices.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use crate::uri::encode_path;

/// Why a file could not be trashed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TrashError {
    /// The path does not exist.
    #[error("the file no longer exists")]
    NotFound,
    /// No trash directory on the file's filesystem can be used.
    #[error("no trash folder is available on this filesystem")]
    NoTrashDir,
    /// The path is a filesystem root or has no file name.
    #[error("this item cannot be moved to the trash")]
    NotTrashable,
    /// No permission, read-only filesystem or another I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Moves `path` to the trash and returns where it now lives (inside the trash's `files` folder).
///
/// Blocking filesystem I/O: call from a worker thread.
///
/// # Errors
///
/// See [`TrashError`]. On failure the file is left where it was.
pub fn trash(path: &Path) -> Result<PathBuf, TrashError> {
    let home_data = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".local/share"))
        })
        .ok_or(TrashError::NoTrashDir)?;
    trash_with(path, &home_data.join("Trash"), rustix::process::getuid().as_raw())
}

/// [`trash`] with an explicit home trash directory and user id (for tests).
pub(crate) fn trash_with(path: &Path, home_trash: &Path, uid: u32) -> Result<PathBuf, TrashError> {
    let abs = std::path::absolute(path)?;
    let meta = fs::symlink_metadata(&abs).map_err(not_found)?;
    if abs.file_name().is_none() {
        return Err(TrashError::NotTrashable);
    }
    let (trash_dir, info_base) = choose_trash(&abs, meta.dev(), home_trash, uid)?;
    let files = trash_dir.join("files");
    let info = trash_dir.join("info");
    for dir in [&files, &info] {
        create_private_dir(dir)?;
    }
    let recorded = match &info_base {
        Some(top) => abs.strip_prefix(top).unwrap_or(&abs).to_path_buf(),
        None => abs.clone(),
    };
    let name = abs.file_name().unwrap_or_default();
    let date = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
    let content = format!(
        "[Trash Info]\nPath={}\nDeletionDate={date}\n",
        encode_path(&recorded)
    );

    for attempt in 1..=1000u32 {
        let mut candidate = name.to_os_string();
        if attempt > 1 {
            candidate.push(format!(".{attempt}"));
        }
        let target = files.join(&candidate);
        if fs::symlink_metadata(&target).is_ok() {
            continue;
        }
        let mut info_name = candidate.clone();
        info_name.push(".trashinfo");
        let info_path = info.join(info_name);
        // `create_new` claims the name atomically: two trashers cannot pick the same one.
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&info_path) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        };
        if let Err(e) = file.write_all(content.as_bytes()).and_then(|()| file.sync_all()) {
            let _ = fs::remove_file(&info_path);
            return Err(e.into());
        }
        return match fs::rename(&abs, &target) {
            Ok(()) => Ok(target),
            Err(e) => {
                let _ = fs::remove_file(&info_path);
                Err(e.into())
            }
        };
    }
    Err(TrashError::NoTrashDir)
}

fn not_found(e: std::io::Error) -> TrashError {
    if e.kind() == ErrorKind::NotFound {
        TrashError::NotFound
    } else {
        TrashError::Io(e)
    }
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    match fs::DirBuilder::new().recursive(true).mode(0o700).create(dir) {
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(()),
        other => other,
    }
}

/// The trash directory for a file on device `dev`, and the topdir that `Path=` is relative to
/// (`None` for the home trash, whose `Path=` is absolute).
fn choose_trash(
    abs: &Path,
    dev: u64,
    home_trash: &Path,
    uid: u32,
) -> Result<(PathBuf, Option<PathBuf>), TrashError> {
    // The home trash may not exist yet: its parent decides which device it will live on.
    let home_dev = nearest_existing(home_trash)
        .and_then(|p| fs::metadata(p).ok())
        .map(|m| m.dev());
    if home_dev == Some(dev) {
        return Ok((home_trash.to_path_buf(), None));
    }
    let top = mount_point(abs, dev);
    let shared = top.join(".Trash");
    if let Ok(m) = fs::symlink_metadata(&shared)
        && m.is_dir()
        && !m.file_type().is_symlink()
        && m.permissions().mode() & 0o1000 != 0
    {
        let own = shared.join(uid.to_string());
        if create_private_dir(&own).is_ok() {
            return Ok((own, Some(top)));
        }
    }
    let own = top.join(format!(".Trash-{uid}"));
    create_private_dir(&own).map_err(|_| TrashError::NoTrashDir)?;
    Ok((own, Some(top)))
}

fn nearest_existing(path: &Path) -> Option<&Path> {
    path.ancestors().find(|p| p.exists())
}

/// The highest ancestor of `path` that is still on device `dev`.
fn mount_point(path: &Path, dev: u64) -> PathBuf {
    let mut top = path.parent().unwrap_or(Path::new("/")).to_path_buf();
    while let Some(parent) = top.parent() {
        match fs::metadata(parent) {
            Ok(m) if m.dev() == dev => top = parent.to_path_buf(),
            _ => break,
        }
    }
    top
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let trash = tmp.path().join("share/Trash");
        let data = tmp.path().join("data");
        fs::create_dir_all(&data).expect("data dir");
        (tmp, trash, data)
    }

    fn only_info(trash: &Path) -> String {
        let mut entries: Vec<_> = fs::read_dir(trash.join("info"))
            .expect("info dir")
            .flatten()
            .collect();
        assert_eq!(entries.len(), 1, "one trashinfo");
        fs::read_to_string(entries.remove(0).path()).expect("info text")
    }

    #[test]
    fn moves_the_file_and_records_where_it_came_from() {
        let (_tmp, trash, data) = setup();
        let file = data.join("my file#1.txt");
        fs::write(&file, "hello").expect("write");
        let moved = trash_with(&file, &trash, 1000).expect("trashed");
        assert!(!file.exists());
        assert_eq!(moved, trash.join("files/my file#1.txt"));
        assert_eq!(fs::read_to_string(&moved).expect("content"), "hello");
        let info = only_info(&trash);
        assert!(info.starts_with("[Trash Info]\nPath="), "{info}");
        assert!(info.contains("my%20file%231.txt"), "encoded: {info}");
        assert!(info.contains("\nDeletionDate="), "{info}");
        let mode = fs::metadata(trash.join("files"))
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "private trash folder");
    }

    #[test]
    fn name_collisions_get_a_suffix_and_keep_both_files() {
        let (_tmp, trash, data) = setup();
        let a = data.join("a.txt");
        fs::write(&a, "1").expect("write");
        let first = trash_with(&a, &trash, 1000).expect("first");
        fs::write(&a, "2").expect("write");
        let second = trash_with(&a, &trash, 1000).expect("second");
        assert_ne!(first, second);
        assert_eq!(fs::read_to_string(first).expect("first"), "1");
        assert_eq!(fs::read_to_string(second).expect("second"), "2");
    }

    #[test]
    fn folders_and_dangling_symlinks_can_be_trashed() {
        let (_tmp, trash, data) = setup();
        let dir = data.join("dir");
        fs::create_dir_all(dir.join("sub")).expect("dirs");
        fs::write(dir.join("sub/f"), "x").expect("file");
        trash_with(&dir, &trash, 1000).expect("dir trashed");
        assert!(trash.join("files/dir/sub/f").exists());
        let link = data.join("dangling");
        std::os::unix::fs::symlink("/nonexistent/target", &link).expect("symlink");
        trash_with(&link, &trash, 1000).expect("link trashed");
        assert!(fs::symlink_metadata(trash.join("files/dangling")).is_ok());
    }

    #[test]
    fn missing_files_report_not_found() {
        let (_tmp, trash, data) = setup();
        let err = trash_with(&data.join("nope"), &trash, 1000).expect_err("missing");
        assert!(matches!(err, TrashError::NotFound), "{err:?}");
    }

    #[test]
    fn invalid_utf8_names_survive() {
        use std::os::unix::ffi::OsStrExt as _;
        let (_tmp, trash, data) = setup();
        let file = data.join(std::ffi::OsStr::from_bytes(b"bad\xffname"));
        fs::write(&file, "x").expect("write");
        trash_with(&file, &trash, 1000).expect("trashed");
        assert!(only_info(&trash).contains("bad%FFname"));
    }
}

//! File operations started from the UI (ROADMAP-v0.5 Phase D). Blocking: call from a worker thread.

use std::ffi::OsStr;
use std::fs::File;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use rustix::fs::{RenameFlags, renameat, renameat_with};
use rustix::io::Errno;

/// Longest file name in bytes on ext4 (`NAME_MAX`).
const NAME_MAX: usize = 255;

/// Why a rename failed. Each variant has a user-facing message (`Display`).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RenameError {
    /// The new name is empty, `.`/`..`, contains `/` or NUL, or is longer than 255 bytes.
    #[error("invalid name: {0}")]
    InvalidName(&'static str),
    /// Another file or folder already has the new name (nothing was overwritten).
    #[error("a file or folder with that name already exists")]
    AlreadyExists,
    /// No permission to rename in this folder.
    #[error("permission denied")]
    PermissionDenied,
    /// The file no longer exists.
    #[error("the file no longer exists")]
    NotFound,
    /// The item is a mount point or otherwise busy.
    #[error("the item is in use (for example a mount point)")]
    Busy,
    /// The filesystem is mounted read-only.
    #[error("the filesystem is read-only")]
    ReadOnlyFilesystem,
    /// Anything else.
    #[error(transparent)]
    Other(std::io::Error),
}

/// Validates `new_name` as a single path component (no filesystem access).
///
/// Operates on bytes, so names that are not valid UTF-8 are accepted.
pub fn validate_name(new_name: &OsStr) -> Result<(), RenameError> {
    let bytes = new_name.as_bytes();
    if bytes.is_empty() {
        return Err(RenameError::InvalidName("the name is empty"));
    }
    if bytes == b"." || bytes == b".." {
        return Err(RenameError::InvalidName("\".\" and \"..\" are reserved names"));
    }
    if bytes.contains(&b'/') {
        return Err(RenameError::InvalidName("the name contains \"/\""));
    }
    if bytes.contains(&0) {
        return Err(RenameError::InvalidName("the name contains a NUL byte"));
    }
    if bytes.len() > NAME_MAX {
        return Err(RenameError::InvalidName("the name is longer than 255 bytes"));
    }
    Ok(())
}

fn map_errno(errno: Errno) -> RenameError {
    match errno {
        Errno::EXIST | Errno::NOTEMPTY => RenameError::AlreadyExists,
        Errno::ACCESS | Errno::PERM => RenameError::PermissionDenied,
        Errno::NOENT => RenameError::NotFound,
        Errno::BUSY => RenameError::Busy,
        Errno::ROFS => RenameError::ReadOnlyFilesystem,
        other => RenameError::Other(other.into()),
    }
}

/// Renames `path` to `new_name` inside the same folder, atomically and **never overwriting** an
/// existing entry (`renameat2(RENAME_NOREPLACE)`). Returns the new path (the parent of `path`
/// joined with `new_name`; absolute when `path` is).
///
/// This is **blocking** filesystem I/O: call it from a worker thread, never from the UI thread.
///
/// If `new_name` equals the current name, returns `path` unchanged without touching the
/// filesystem. If the filesystem does not support `RENAME_NOREPLACE` (`EINVAL`/`ENOSYS`), falls
/// back to an existence check followed by a plain `rename`. That fallback has a race: an entry
/// created between the check and the rename would be silently replaced.
pub fn rename_in_place(path: &Path, new_name: &OsStr) -> Result<PathBuf, RenameError> {
    validate_name(new_name)?;
    let old_name = path
        .file_name()
        .ok_or(RenameError::InvalidName("the path has no file name"))?;
    if old_name == new_name {
        return Ok(path.to_path_buf());
    }
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let dir = File::open(parent).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => RenameError::NotFound,
        std::io::ErrorKind::PermissionDenied => RenameError::PermissionDenied,
        _ => RenameError::Other(e),
    })?;
    let target = parent.join(new_name);
    match renameat_with(&dir, old_name, &dir, new_name, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(target),
        Err(Errno::INVAL | Errno::NOSYS) => {
            // No NOREPLACE support. Racy check-then-rename; a missing source still yields ENOENT.
            match std::fs::symlink_metadata(&target) {
                Ok(_) => return Err(RenameError::AlreadyExists),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(RenameError::Other(e)),
            }
            renameat(&dir, old_name, &dir, new_name).map_err(map_errno)?;
            Ok(target)
        }
        Err(e) => Err(map_errno(e)),
    }
}

/// Outcome of pasting one source: its new path, or why it failed.
pub type PasteResult = (PathBuf, Result<PathBuf, String>);

/// Permanently deletes every path of `paths`: files and symlinks are unlinked, folders are removed
/// with their contents. A symlink is removed itself, never followed.
///
/// One result per path, in order; a failure does not stop the others.
///
/// This is **blocking** filesystem I/O: call it from a worker thread.
pub fn delete_paths(paths: &[PathBuf]) -> Vec<(PathBuf, Result<(), String>)> {
    paths
        .iter()
        .map(|path| {
            let result = std::fs::symlink_metadata(path).and_then(|meta| {
                if meta.is_dir() {
                    std::fs::remove_dir_all(path)
                } else {
                    std::fs::remove_file(path)
                }
            });
            (path.clone(), result.map_err(|e| e.to_string()))
        })
        .collect()
}

/// Copies (or, with `cut`, moves) every path of `sources` into the folder `dest`.
///
/// Nothing is ever overwritten: a name that is taken gets ` (copy)`, ` (copy 2)`, … before the
/// extension. Folders are copied recursively and symlinks are recreated, not followed. A move
/// within one filesystem is a rename; across filesystems it copies and then removes the source.
/// Moving an item into the folder it is already in does nothing. A folder cannot be pasted into
/// itself or one of its subfolders.
///
/// One result per source, in order; a failure does not stop the others.
///
/// This is **blocking** filesystem I/O: call it from a worker thread.
pub fn paste_into(dest: &Path, sources: &[PathBuf], cut: bool) -> Vec<PasteResult> {
    sources
        .iter()
        .map(|src| {
            let result = paste_one(dest, src, cut).map_err(|e| e.to_string());
            (src.clone(), result)
        })
        .collect()
}

fn paste_one(dest: &Path, src: &Path, cut: bool) -> std::io::Result<PathBuf> {
    use std::io::{Error, ErrorKind};

    let meta = std::fs::symlink_metadata(src)?;
    let name = src
        .file_name()
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "the path has no file name"))?;
    if !std::fs::metadata(dest)?.is_dir() {
        return Err(Error::new(ErrorKind::NotADirectory, "the target is not a folder"));
    }
    if meta.is_dir() && std::fs::canonicalize(dest)?.starts_with(std::fs::canonicalize(src)?) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "a folder cannot be pasted into itself",
        ));
    }
    if cut && src.parent() == Some(dest) {
        return Ok(src.to_path_buf());
    }
    let target = unique_target(dest, name)?;
    if cut {
        match renameat_with(
            rustix::fs::CWD,
            src,
            rustix::fs::CWD,
            &target,
            RenameFlags::NOREPLACE,
        ) {
            Ok(()) => return Ok(target),
            Err(Errno::XDEV) => {}
            Err(Errno::INVAL | Errno::NOSYS) => {
                std::fs::rename(src, &target)?;
                return Ok(target);
            }
            Err(e) => return Err(e.into()),
        }
    }
    copy_tree(src, &target)?;
    if cut {
        if meta.is_dir() {
            std::fs::remove_dir_all(src)?;
        } else {
            std::fs::remove_file(src)?;
        }
    }
    Ok(target)
}

/// `dest/name`, or the first free `stem (copy).ext`, `stem (copy 2).ext`, ….
fn unique_target(dest: &Path, name: &OsStr) -> std::io::Result<PathBuf> {
    let taken = |p: &Path| std::fs::symlink_metadata(p).is_ok();
    let first = dest.join(name);
    if !taken(&first) {
        return Ok(first);
    }
    let bytes = name.as_bytes();
    // The extension starts at the last dot that is not the first byte.
    let split = bytes
        .iter()
        .rposition(|&b| b == b'.')
        .filter(|&i| i > 0)
        .unwrap_or(bytes.len());
    let (stem, ext) = bytes.split_at(split);
    for n in 1..=9999 {
        let mut candidate = stem.to_vec();
        candidate.extend_from_slice(
            if n == 1 {
                " (copy)".to_owned()
            } else {
                format!(" (copy {n})")
            }
            .as_bytes(),
        );
        candidate.extend_from_slice(ext);
        let path = dest.join(OsStr::from_bytes(&candidate));
        if !taken(&path) {
            return Ok(path);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no free name for the copy",
    ))
}

fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(src)?, dst)
    } else if meta.is_dir() {
        std::fs::create_dir(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_tree(&entry.path(), &dst.join(entry.file_name()))?;
        }
        std::fs::set_permissions(dst, meta.permissions())
    } else if meta.is_file() {
        std::fs::copy(src, dst).map(|_| ())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "special files cannot be copied",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;

    #[test]
    fn delete_paths_removes_files_folders_and_links_without_following() {
        let dir = tempfile::tempdir().unwrap();
        let keep = dir.path().join("keep");
        fs::create_dir(&keep).unwrap();
        fs::write(keep.join("data"), b"x").unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, b"x").unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        fs::write(tree.join("sub/b"), b"x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&keep, &link).unwrap();
        let missing = dir.path().join("missing");

        let results = delete_paths(&[file.clone(), tree.clone(), link.clone(), missing.clone()]);
        assert!(results[..3].iter().all(|(_, r)| r.is_ok()));
        assert!(results[3].1.is_err());
        assert!(!file.exists() && !tree.exists());
        assert!(fs::symlink_metadata(&link).is_err());
        assert!(keep.join("data").exists(), "the link target must survive");
    }

    fn invalid(name: &OsStr) -> bool {
        matches!(validate_name(name), Err(RenameError::InvalidName(_)))
    }

    #[test]
    fn validate_rejects_bad_names() {
        for bad in ["", ".", "..", "a/b", "/", "a\0b"] {
            assert!(invalid(OsStr::new(bad)), "{bad:?}");
        }
        assert!(validate_name(OsStr::new("ok.txt")).is_ok());
        assert!(validate_name(OsStr::new("...")).is_ok());
    }

    #[test]
    fn validate_name_length_limit() {
        assert!(validate_name(OsStr::new(&"a".repeat(255))).is_ok());
        assert!(invalid(OsStr::new(&"a".repeat(256))));
        // 128 two-byte characters = 256 bytes.
        assert!(invalid(OsStr::new(&"é".repeat(128))));
    }

    #[test]
    fn renames_file() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("a.txt");
        fs::write(&old, "hello").unwrap();
        let new = rename_in_place(&old, OsStr::new("b.txt")).unwrap();
        assert_eq!(new, dir.path().join("b.txt"));
        assert!(!old.exists());
        assert_eq!(fs::read_to_string(&new).unwrap(), "hello");
    }

    #[test]
    fn renames_folder_with_children() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("d");
        fs::create_dir_all(old.join("sub")).unwrap();
        fs::write(old.join("sub/f"), "x").unwrap();
        fs::write(old.join("g"), "y").unwrap();
        let new = rename_in_place(&old, OsStr::new("e")).unwrap();
        assert!(!old.exists());
        assert_eq!(fs::read_to_string(new.join("sub/f")).unwrap(), "x");
        assert_eq!(fs::read_to_string(new.join("g")).unwrap(), "y");
    }

    #[test]
    fn same_name_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        // The source does not exist: a syscall would fail with NotFound.
        let p = dir.path().join("ghost");
        assert_eq!(rename_in_place(&p, OsStr::new("ghost")).unwrap(), p);
    }

    #[test]
    fn existing_target_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::write(&a, "A").unwrap();
        fs::write(&b, "B").unwrap();
        let err = rename_in_place(&a, OsStr::new("b")).unwrap_err();
        assert!(matches!(err, RenameError::AlreadyExists), "{err:?}");
        assert_eq!(fs::read_to_string(&a).unwrap(), "A");
        assert_eq!(fs::read_to_string(&b).unwrap(), "B");
    }

    #[test]
    fn existing_folder_target_is_already_exists() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let err = rename_in_place(&a, OsStr::new("b")).unwrap_err();
        assert!(matches!(err, RenameError::AlreadyExists), "{err:?}");
    }

    #[test]
    fn missing_source_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = rename_in_place(&dir.path().join("nope"), OsStr::new("x")).unwrap_err();
        assert!(matches!(err, RenameError::NotFound), "{err:?}");
        let err = rename_in_place(&dir.path().join("no/dir/f"), OsStr::new("x")).unwrap_err();
        assert!(matches!(err, RenameError::NotFound), "{err:?}");
    }

    #[test]
    fn invalid_new_names_are_rejected_before_any_syscall() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        fs::write(&a, "A").unwrap();
        for bad in ["", ".", "..", "x/y", "x\0y"] {
            let err = rename_in_place(&a, OsStr::new(bad)).unwrap_err();
            assert!(matches!(err, RenameError::InvalidName(_)), "{bad:?}");
        }
        assert!(a.exists());
    }

    #[test]
    fn name_length_boundary_on_rename() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        fs::write(&a, "A").unwrap();
        let err = rename_in_place(&a, OsStr::new(&"n".repeat(256))).unwrap_err();
        assert!(matches!(err, RenameError::InvalidName(_)));
        let new = rename_in_place(&a, OsStr::new(&"n".repeat(255))).unwrap();
        assert!(new.exists());
    }

    #[test]
    fn non_utf8_old_and_new_names() {
        let dir = tempfile::tempdir().unwrap();
        let old_name = OsString::from_vec_for_test(b"old\xff\xfe.bin");
        let new_name = OsString::from_vec_for_test(b"new\x80name");
        let old = dir.path().join(&old_name);
        fs::write(&old, "bytes").unwrap();
        let new = rename_in_place(&old, &new_name).unwrap();
        assert!(!old.exists());
        assert_eq!(new.file_name().unwrap(), new_name);
        assert_eq!(fs::read_to_string(&new).unwrap(), "bytes");
        // And back to a UTF-8 name.
        let back = rename_in_place(&new, OsStr::new("plain")).unwrap();
        assert!(back.exists());
    }

    #[test]
    fn permission_denied_in_readonly_folder() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            eprintln!("skipped: running as root bypasses permission checks");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("ro");
        fs::create_dir(&sub).unwrap();
        let f = sub.join("f");
        fs::write(&f, "x").unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o555)).unwrap();
        let res = rename_in_place(&f, OsStr::new("g"));
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(res, Err(RenameError::PermissionDenied)), "{res:?}");
    }

    /// Helper so tests can build non-UTF-8 `OsString`s without importing the trait everywhere.
    trait FromVecForTest {
        fn from_vec_for_test(v: &[u8]) -> OsString;
    }
    impl FromVecForTest for OsString {
        fn from_vec_for_test(v: &[u8]) -> OsString {
            use std::os::unix::ffi::OsStringExt;
            OsString::from_vec(v.to_vec())
        }
    }

    #[test]
    fn paste_copies_moves_and_never_overwrites() {
        let dir = tempfile_dir();
        let (a, b) = (dir.join("a"), dir.join("b"));
        fs::create_dir_all(a.join("sub")).expect("mkdir");
        fs::create_dir(&b).expect("mkdir");
        fs::write(a.join("f.txt"), "one").expect("write");
        fs::write(a.join("sub/g.txt"), "two").expect("write");
        fs::write(b.join("f.txt"), "taken").expect("write");

        let out = paste_into(&b, &[a.join("f.txt"), a.join("sub")], false);
        assert_eq!(out[0].1.as_ref().expect("copied"), &b.join("f (copy).txt"));
        assert_eq!(fs::read_to_string(b.join("f.txt")).expect("read"), "taken");
        assert_eq!(fs::read_to_string(b.join("sub/g.txt")).expect("read"), "two");
        assert!(a.join("f.txt").exists(), "copy keeps the source");

        let out = paste_into(&b, &[a.join("f.txt")], true);
        assert_eq!(out[0].1.as_ref().expect("moved"), &b.join("f (copy 2).txt"));
        assert!(!a.join("f.txt").exists(), "move removes the source");

        let out = paste_into(&a, &[a.join("sub")], false);
        assert!(out[0].1.is_ok());
        let into_self = paste_into(&a.join("sub"), &[a.join("sub")], false);
        assert!(into_self[0].1.is_err());
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "everythux-paste-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }
}

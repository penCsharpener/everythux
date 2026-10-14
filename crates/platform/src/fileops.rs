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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;

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
}

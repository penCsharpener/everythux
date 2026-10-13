//! Everything written to stdout. Command logic hands data to this module; text is built by
//! [`crate::render`].

#![expect(clippy::print_stdout, reason = "CLI output")]

use std::io::{self, Write};

use everythux_core::ids::EntryId;
use everythux_core::snapshot::IndexSnapshot;
use serde_json::json;

use crate::render;

/// How `search` prints results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultFormat {
    /// One path per line.
    Lines,
    /// One path per NUL byte.
    Null,
    /// One JSON object per line.
    Json,
}

/// Prints a block of text (already newline-terminated) or a single line.
pub(crate) fn text(text: &str) {
    print!("{text}");
}

/// Prints one line.
pub(crate) fn line(line: &str) {
    println!("{line}");
}

/// Writes the full paths of `ids` exactly as stored (raw bytes, so non-UTF-8 names survive), or one
/// JSON object per entry.
pub(crate) fn write_results<W: Write>(
    out: &mut W,
    snapshot: &IndexSnapshot,
    ids: &[EntryId],
    format: ResultFormat,
) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt as _;

    for &id in ids {
        let full = snapshot.full_path(id);
        match format {
            ResultFormat::Lines | ResultFormat::Null => {
                out.write_all(full.as_os_str().as_bytes())?;
                out.write_all(if format == ResultFormat::Null {
                    b"\0"
                } else {
                    b"\n"
                })?;
            }
            ResultFormat::Json => {
                let i = id.index();
                let object = json!({
                    "name": String::from_utf8_lossy(snapshot.name(id)),
                    "path": snapshot.parent_path(id).to_string_lossy(),
                    "full_path": full.to_string_lossy(),
                    "size": snapshot.entries.size[i],
                    "modified": render::rfc3339(snapshot.entries.mtime_ns[i]),
                    "kind": render::kind_name(snapshot.entries.kind[i]),
                });
                serde_json::to_writer(&mut *out, &object)?;
                out.write_all(b"\n")?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    use everythux_core::ids::{DirId, VolumeId};
    use everythux_core::model::EntryKind;
    use everythux_core::snapshot::ByteArena;

    use super::*;

    /// Two entries in `/data/dir`: `a.txt` (file) and `bad\xffname` (folder).
    fn snapshot() -> IndexSnapshot {
        let mut s = IndexSnapshot::empty();
        // `ByteArena::default()` (used by `empty()`) lacks the leading 0 offset; `new()` has it.
        s.dirs.paths = ByteArena::new();
        s.entries.names = ByteArena::new();
        s.entries.names_fold = ByteArena::new();
        s.dirs.paths.push(b"/data/dir");
        s.dirs.parent.push(DirId::NONE);
        s.dirs.volume.push(VolumeId(0));
        s.dirs.total_size.push(0);
        s.dirs.mtime_ns.push(0);
        s.dirs.entry.push(EntryId::NONE);
        for (name, kind, size, mtime) in [
            (
                &b"a.txt"[..],
                EntryKind::File,
                1536_u64,
                1_767_323_045_000_000_000_i64,
            ),
            (&b"bad\xffname"[..], EntryKind::Dir, 0, 0),
        ] {
            s.entries.names.push(name);
            s.entries.names_fold.push(name);
            s.entries.dir.push(DirId(0));
            s.entries.kind.push(kind);
            s.entries.size.push(size);
            s.entries.mtime_ns.push(mtime);
            s.entries.ext.push(0);
            s.entries.self_dir.push(DirId::NONE);
        }
        s
    }

    fn render(format: ResultFormat) -> Vec<u8> {
        let mut out = Vec::new();
        write_results(&mut out, &snapshot(), &[EntryId(0), EntryId(1)], format).unwrap();
        out
    }

    #[test]
    fn lines_keep_non_utf8_bytes_exact() {
        let out = render(ResultFormat::Lines);
        let mut expected = b"/data/dir/a.txt\n/data/dir/bad".to_vec();
        expected.extend_from_slice(b"\xffname\n");
        assert_eq!(out, expected);
        assert_eq!(OsStr::from_bytes(&out[..15]), OsStr::new("/data/dir/a.txt"));
    }

    #[test]
    fn null_format_separates_with_nul() {
        let out = render(ResultFormat::Null);
        let parts: Vec<&[u8]> = out.split(|&b| b == 0).collect();
        assert_eq!(parts, [&b"/data/dir/a.txt"[..], b"/data/dir/bad\xffname", b""]);
    }

    #[test]
    fn json_objects_have_the_documented_fields() {
        let out = String::from_utf8(render(ResultFormat::Json)).unwrap();
        let rows: Vec<serde_json::Value> = out.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(
            rows[0],
            json!({
                "name": "a.txt",
                "path": "/data/dir",
                "full_path": "/data/dir/a.txt",
                "size": 1536,
                "modified": "2026-01-02T03:04:05Z",
                "kind": "file",
            })
        );
        assert_eq!(rows[1]["name"], "bad\u{fffd}name");
        assert_eq!(rows[1]["kind"], "dir");
    }
}

//! Text / source / config previews: the head of the file, and further chunks on request, decoded
//! from whatever encoding [`crate::encoding::detect`] finds.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::cancel::Cancel;
use crate::encoding::{self, Strictness, TextEncoding};
use crate::error::PreviewError;
use crate::{PreviewContent, TEXT_HEAD_BYTES, TEXT_MAX_BYTES};

/// Longest encoded character (UTF-8 and UTF-32: 4 bytes).
const MAX_CHAR_BYTES: u64 = 4;

/// Reads the head of `path` as text.
///
/// With `sniff` (unknown file type) the content must look like text; known text extensions are
/// more lenient (see [`Strictness`]). Binary content is [`PreviewError::NotText`] when sniffing
/// and [`PreviewError::Binary`] otherwise.
pub(crate) fn read_text(path: &Path, sniff: bool, cancel: &Cancel) -> Result<PreviewContent, PreviewError> {
    cancel.check()?;
    let mut file = File::open(path)?;
    let total = file.metadata()?.len();
    let mut buf = Vec::with_capacity(total.min(TEXT_HEAD_BYTES) as usize);
    file.by_ref().take(TEXT_HEAD_BYTES).read_to_end(&mut buf)?;
    let read = buf.len() as u64;
    let total = total.max(read);
    let at_eof = read < TEXT_HEAD_BYTES || total == read;

    let strictness = if sniff {
        Strictness::Sniff
    } else {
        Strictness::KnownText
    };
    let Some(found) = encoding::detect(&buf, at_eof, strictness) else {
        return Err(if sniff {
            PreviewError::NotText
        } else {
            PreviewError::Binary
        });
    };
    cancel.check()?;
    let (text, consumed) = encoding::decode(&buf[found.bom_len..], found.encoding, at_eof);
    Ok(text_content(
        text,
        found.encoding,
        total,
        (found.bom_len + consumed) as u64,
    ))
}

/// Reads up to `len` bytes of `path` from `offset` (a previous chunk's `end_offset`) as more text
/// in `encoding`. Nothing past [`TEXT_MAX_BYTES`] is ever read.
pub(crate) fn read_range(
    path: &Path,
    offset: u64,
    len: u64,
    encoding: TextEncoding,
    cancel: &Cancel,
) -> Result<PreviewContent, PreviewError> {
    cancel.check()?;
    if offset >= TEXT_MAX_BYTES {
        return Err(PreviewError::TextCap);
    }
    let len = len.min(TEXT_MAX_BYTES - offset);
    let mut file = File::open(path)?;
    let total = file.metadata()?.len();
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::with_capacity(len.min(total.saturating_sub(offset)) as usize);
    file.take(len).read_to_end(&mut buf)?;
    cancel.check()?;
    let end = offset + buf.len() as u64;
    let total = total.max(end);
    // At the cap the rest is never read, so a character cut there is decoded (as U+FFFD) rather
    // than kept for a chunk that will not come.
    let at_eof = end >= total || end >= TEXT_MAX_BYTES;
    let (text, consumed) = encoding::decode(&buf, encoding, at_eof);
    Ok(text_content(text, encoding, total, offset + consumed as u64))
}

fn text_content(text: String, encoding: TextEncoding, total_bytes: u64, end_offset: u64) -> PreviewContent {
    PreviewContent::Text {
        text,
        truncated: end_offset < total_bytes,
        encoding,
        total_bytes,
        end_offset,
    }
}

/// Whether a text preview that ends at `end_offset` of a `total_bytes` file can be extended with
/// [`crate::PreviewService::request_range`] (more of the file exists and the cap is not reached).
pub fn more_text_available(end_offset: u64, total_bytes: u64) -> bool {
    end_offset < total_bytes && end_offset + MAX_CHAR_BYTES <= TEXT_MAX_BYTES
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).expect("write");
        path
    }

    fn read(dir: &Path, name: &str, bytes: &[u8], sniff: bool) -> Result<PreviewContent, PreviewError> {
        read_text(&write(dir, name, bytes), sniff, &Cancel::never())
    }

    fn text_of(content: PreviewContent) -> (String, bool, TextEncoding, u64, u64) {
        match content {
            PreviewContent::Text {
                text,
                truncated,
                encoding,
                total_bytes,
                end_offset,
            } => (text, truncated, encoding, total_bytes, end_offset),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn small_file_is_complete() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let content = read(tmp.path(), "a.txt", "héllo\nwörld\n".as_bytes(), false).expect("text");
        assert_eq!(content, PreviewContent::text("héllo\nwörld\n"));
    }

    #[test]
    fn large_file_is_cut_at_the_head_size() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let head = TEXT_HEAD_BYTES as usize;
        let big = vec![b'a'; head + 10];
        let (text, truncated, _, total, end) =
            text_of(read(tmp.path(), "big.log", &big, false).expect("text"));
        assert!(truncated);
        assert_eq!((text.len(), total, end), (head, head as u64 + 10, head as u64));

        let exact = vec![b'a'; head];
        let (_, truncated, ..) = text_of(read(tmp.path(), "exact.log", &exact, false).expect("text"));
        assert!(!truncated, "a file of exactly the head size is complete");
    }

    #[test]
    fn cut_inside_a_multibyte_char_does_not_add_replacement_char() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let head = TEXT_HEAD_BYTES as usize;
        // "ä" is two bytes; make the head boundary fall in the middle of one.
        let mut data = vec![b'a'; head - 1];
        data.extend("ä".as_bytes());
        data.extend(b"more");
        let path = write(tmp.path(), "u.txt", &data);
        let (text, truncated, encoding, _, end) =
            text_of(read_text(&path, false, &Cancel::never()).expect("text"));
        assert!(truncated);
        assert!(!text.contains('\u{fffd}'));
        assert_eq!(text.len(), head - 1);
        assert_eq!(end, head as u64 - 1, "the next chunk starts at the cut character");
        let (rest, truncated, ..) =
            text_of(read_range(&path, end, 1024, encoding, &Cancel::never()).expect("range"));
        assert_eq!(rest, "ämore");
        assert!(!truncated);
    }

    #[test]
    fn latin1_text_is_decoded_as_windows_1252() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (text, _, encoding, ..) =
            text_of(read(tmp.path(), "l1.txt", b"caf\xe9 au lait", false).expect("text"));
        assert_eq!(
            (text.as_str(), encoding),
            ("café au lait", TextEncoding::Windows1252)
        );
    }

    #[test]
    fn sniffing_rejects_binary_and_accepts_unknown_text() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            read(tmp.path(), "blob", &[0x7f, b'E', b'L', b'F', 0, 1, 2, 3], true),
            Err(PreviewError::NotText)
        ));
        let noise: Vec<u8> = (0..4096u32).map(|i| (128 + i % 100) as u8).collect();
        assert!(matches!(
            read(tmp.path(), "noise", &noise, true),
            Err(PreviewError::NotText)
        ));
        assert!(matches!(
            read(tmp.path(), "Makefile", b"all:\n\tcargo build\n", true),
            Ok(PreviewContent::Text { .. })
        ));
        assert!(matches!(
            read(tmp.path(), "empty", b"", true),
            Ok(PreviewContent::Text { text, .. }) if text.is_empty()
        ));
    }

    #[test]
    fn binary_with_a_text_extension_is_reported_as_binary() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let elf = b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0\x03\0\x3e\0\x01\0\0\0";
        assert!(matches!(
            read(tmp.path(), "renamed.txt", elf, false),
            Err(PreviewError::Binary)
        ));
    }

    #[test]
    fn ranges_continue_where_the_previous_chunk_ended() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let line = "0123456789abcdef\n";
        let data = line.repeat(10_000); // 170 000 bytes
        let path = write(tmp.path(), "r.log", data.as_bytes());
        let (head, _, encoding, total, mut end) =
            text_of(read_text(&path, false, &Cancel::never()).expect("head"));
        assert_eq!(total, data.len() as u64);
        let mut all = head;
        while more_text_available(end, total) {
            let (chunk, _, _, _, next) =
                text_of(read_range(&path, end, 50_000, encoding, &Cancel::never()).expect("range"));
            assert!(next > end, "progress");
            all.push_str(&chunk);
            end = next;
        }
        assert_eq!(all, data);
    }

    #[test]
    fn utf16_ranges_never_split_a_code_unit() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let text: String = "zß😀\n".repeat(30_000);
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend(unit.to_le_bytes());
        }
        let path = write(tmp.path(), "w.txt", &bytes);
        let (mut all, _, encoding, total, mut end) =
            text_of(read_text(&path, false, &Cancel::never()).expect("head"));
        assert_eq!(encoding, TextEncoding::Utf16Le);
        while more_text_available(end, total) {
            // An odd length forces cuts inside code units and surrogate pairs.
            let (chunk, _, _, _, next) =
                text_of(read_range(&path, end, 77_777, encoding, &Cancel::never()).expect("range"));
            all.push_str(&chunk);
            end = next;
        }
        assert_eq!(all, text);
    }

    #[test]
    fn ranges_stop_at_the_cap() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(tmp.path(), "small.txt", b"abc");
        assert!(matches!(
            read_range(&path, TEXT_MAX_BYTES, 10, TextEncoding::Utf8, &Cancel::never()),
            Err(PreviewError::TextCap)
        ));
        assert!(more_text_available(TEXT_MAX_BYTES - 100, TEXT_MAX_BYTES * 2));
        assert!(!more_text_available(TEXT_MAX_BYTES - 2, TEXT_MAX_BYTES * 2));
        assert!(!more_text_available(10, 10));
    }
}

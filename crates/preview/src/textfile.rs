//! Text / source / config previews: the first 64 KiB, lossily decoded.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::PreviewContent;
use crate::cancel::Cancel;
use crate::error::PreviewError;

/// Maximum number of bytes shown.
pub(crate) const TEXT_LIMIT: usize = 64 * 1024;
/// Bytes inspected to decide whether an unknown file is text.
const SNIFF_LEN: usize = 8 * 1024;

/// Reads the head of `path` as text.
///
/// With `sniff`, the file must also look like text (no NUL bytes, mostly valid UTF-8); known text
/// extensions skip that check.
pub(crate) fn read_text(path: &Path, sniff: bool, cancel: &Cancel) -> Result<PreviewContent, PreviewError> {
    cancel.check()?;
    let mut file = File::open(path)?;
    let mut buf = Vec::with_capacity(SNIFF_LEN);
    file.by_ref().take(SNIFF_LEN as u64).read_to_end(&mut buf)?;
    if sniff && !looks_like_text(&buf) {
        return Err(PreviewError::NotText);
    }
    cancel.check()?;
    let want = TEXT_LIMIT + 1 - buf.len().min(TEXT_LIMIT + 1);
    file.take(want as u64).read_to_end(&mut buf)?;

    let truncated = buf.len() > TEXT_LIMIT;
    if truncated {
        buf.truncate(TEXT_LIMIT);
        // Do not end on half a UTF-8 sequence cut by the limit.
        if let Err(e) = std::str::from_utf8(&buf)
            && e.error_len().is_none()
        {
            buf.truncate(e.valid_up_to());
        }
    }
    let text = String::from_utf8(buf).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    Ok(PreviewContent::Text { text, truncated })
}

/// Content sniffing on the first bytes of a file: text has no NUL bytes, is mostly valid UTF-8 and
/// has few control characters.
pub(crate) fn looks_like_text(head: &[u8]) -> bool {
    if head.is_empty() {
        return true;
    }
    if head.contains(&0) {
        return false;
    }
    let cut_at_sniff_boundary = head.len() == SNIFF_LEN;
    let mut invalid = 0usize;
    let mut chunks = head.utf8_chunks().peekable();
    while let Some(chunk) = chunks.next() {
        let tail = chunk.invalid();
        // A multi-byte character split by the sniff window is not corruption.
        let split_tail = cut_at_sniff_boundary && chunks.peek().is_none() && tail.len() < 4;
        if !split_tail {
            invalid += tail.len();
        }
    }
    let control = head
        .iter()
        .filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0c | 0x1b))
        .count();
    invalid * 10 <= head.len() && control * 20 <= head.len()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn read(dir: &Path, name: &str, bytes: &[u8], sniff: bool) -> Result<PreviewContent, PreviewError> {
        let path = dir.join(name);
        fs::write(&path, bytes).expect("write");
        read_text(&path, sniff, &Cancel::never())
    }

    #[test]
    fn small_file_is_not_truncated() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let content = read(tmp.path(), "a.txt", "héllo\nwörld\n".as_bytes(), false).expect("text");
        assert_eq!(
            content,
            PreviewContent::Text {
                text: "héllo\nwörld\n".to_owned(),
                truncated: false
            }
        );
    }

    #[test]
    fn large_file_is_cut_at_64_kib() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let big = vec![b'a'; TEXT_LIMIT + 10];
        let PreviewContent::Text { text, truncated } =
            read(tmp.path(), "big.log", &big, false).expect("text")
        else {
            panic!("expected text");
        };
        assert!(truncated);
        assert_eq!(text.len(), TEXT_LIMIT);

        let exact = vec![b'a'; TEXT_LIMIT];
        let PreviewContent::Text { truncated, .. } =
            read(tmp.path(), "exact.log", &exact, false).expect("text")
        else {
            panic!("expected text");
        };
        assert!(!truncated, "a file of exactly the limit is complete");
    }

    #[test]
    fn cut_inside_a_multibyte_char_does_not_add_replacement_char() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // "ä" is two bytes; make the 64 KiB boundary fall in the middle of one.
        let mut data = vec![b'a'; TEXT_LIMIT - 1];
        data.extend("ä".as_bytes());
        data.extend(b"more");
        let PreviewContent::Text { text, truncated } = read(tmp.path(), "u.txt", &data, false).expect("text")
        else {
            panic!("expected text");
        };
        assert!(truncated);
        assert!(!text.contains('\u{fffd}'));
        assert_eq!(text.len(), TEXT_LIMIT - 1);
    }

    #[test]
    fn invalid_utf8_is_replaced() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let PreviewContent::Text { text, .. } =
            read(tmp.path(), "l1.txt", b"caf\xe9 au lait", false).expect("text")
        else {
            panic!("expected text");
        };
        assert_eq!(text, "caf\u{fffd} au lait");
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
    fn sniffing_tolerates_a_character_split_by_the_window() {
        let mut data = vec![b'a'; SNIFF_LEN - 1];
        data.extend(&"€".as_bytes()[..1]); // first byte of a 3-byte char at the window edge
        assert_eq!(data.len(), SNIFF_LEN);
        assert!(looks_like_text(&data));
    }
}

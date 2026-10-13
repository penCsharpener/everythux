//! Text helpers shared by indexing and querying, so both sides fold names identically.

use std::borrow::Cow;

/// Case-folds a raw filename for case-insensitive matching.
///
/// Invalid UTF-8 is replaced lossily, then the result is Unicode-lowercased. The indexer stores the
/// result as `name_fold`; the search engine folds query terms with the same function.
pub fn fold(raw: &[u8]) -> String {
    let lossy = String::from_utf8_lossy(raw);
    if lossy.bytes().all(|b| b.is_ascii() && !b.is_ascii_uppercase()) {
        return lossy.into_owned();
    }
    if lossy.is_ascii() {
        return lossy.to_ascii_lowercase();
    }
    lossy.to_lowercase()
}

/// Lowercased extension of a raw filename (bytes after the last `.`, if the name doesn't start with
/// it and the dot isn't the last byte). Returns `None` for names without an extension.
pub fn extension(raw: &[u8]) -> Option<Cow<'_, str>> {
    let dot = memrchr_dot(raw)?;
    if dot == 0 || dot + 1 == raw.len() {
        return None;
    }
    let ext = &raw[dot + 1..];
    match String::from_utf8_lossy(ext) {
        Cow::Borrowed(s) if !s.bytes().any(|b| b.is_ascii_uppercase()) => Some(Cow::Borrowed(s)),
        other => Some(Cow::Owned(other.to_lowercase())),
    }
}

fn memrchr_dot(raw: &[u8]) -> Option<usize> {
    raw.iter().rposition(|&b| b == b'.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_lowercases_and_handles_invalid_utf8() {
        assert_eq!(fold(b"ReadMe.MD"), "readme.md");
        assert_eq!(fold("ÄPFEL.txt".as_bytes()), "äpfel.txt");
        assert_eq!(fold(b"bad\xffname"), "bad\u{fffd}name");
        assert_eq!(fold(b"already"), "already");
    }

    #[test]
    fn extension_rules() {
        assert_eq!(extension(b"a.PDF").as_deref(), Some("pdf"));
        assert_eq!(extension(b"archive.tar.gz").as_deref(), Some("gz"));
        assert_eq!(extension(b".bashrc"), None);
        assert_eq!(extension(b"noext"), None);
        assert_eq!(extension(b"trailing."), None);
    }
}

//! Text encodings: detection on the head of a file, and decoding of any chunk of it.
//!
//! Covers what developers meet in practice without a dependency: UTF-8 (with or without BOM),
//! UTF-16 and UTF-32 (BOM, or the NUL pattern of mostly-ASCII text), and Windows-1252 for legacy
//! 8-bit files. Everything else counts as binary.

use std::fmt;

/// How a text file is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TextEncoding {
    /// UTF-8 (a BOM, if any, is skipped).
    Utf8,
    /// UTF-16, little endian.
    Utf16Le,
    /// UTF-16, big endian.
    Utf16Be,
    /// UTF-32, little endian.
    Utf32Le,
    /// UTF-32, big endian.
    Utf32Be,
    /// Windows-1252 (a superset of Latin-1 for printable characters).
    Windows1252,
}

impl TextEncoding {
    /// Display name, e.g. `"UTF-16 LE"`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
            Self::Utf32Le => "UTF-32 LE",
            Self::Utf32Be => "UTF-32 BE",
            Self::Windows1252 => "Windows-1252",
        }
    }
}

impl fmt::Display for TextEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How hard [`detect`] looks before calling a file text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Strictness {
    /// Unknown file type: no NUL bytes outside UTF-16/32, few control characters.
    Sniff,
    /// Known text extension or name: tolerates a few stray NULs (crash-truncated logs) and
    /// control characters.
    KnownText,
}

/// An encoding and the length of the byte order mark in front of the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Detected {
    pub(crate) encoding: TextEncoding,
    pub(crate) bom_len: usize,
}

/// Bytes the UTF-16/32 NUL-pattern heuristic looks at.
const PATTERN_WINDOW: usize = 8 * 1024;

/// Detects the encoding of `head`, the first bytes of a file. `at_eof` says whether `head` is
/// the whole file (otherwise a character cut at its end is not an error). `None` means binary.
pub(crate) fn detect(head: &[u8], at_eof: bool, strictness: Strictness) -> Option<Detected> {
    if let Some(found) = by_bom(head) {
        return Some(found);
    }
    if head.is_empty() {
        return Some(plain(TextEncoding::Utf8));
    }
    if let Some(encoding) = by_nul_pattern(&head[..head.len().min(PATTERN_WINDOW)]) {
        let window = &head[..head.len().min(PATTERN_WINDOW)];
        let (text, _) = decode(window, encoding, at_eof);
        return (control_ratio_ok(&text, strictness) && !text.contains('\0')).then(|| plain(encoding));
    }

    let nuls = memchr_count(0, head);
    let nul_limit = match strictness {
        Strictness::Sniff => 0,
        Strictness::KnownText => head.len() / 100,
    };
    if nuls > nul_limit {
        return None;
    }

    let utf8 = Utf8Stats::of(head, at_eof);
    let encoding = if utf8.invalid == 0 || utf8.multibyte > utf8.invalid {
        // Valid UTF-8, or UTF-8 with a few broken bytes: decoding it as 8-bit would garble every
        // correct multi-byte character.
        TextEncoding::Utf8
    } else if utf8.invalid * 10 < head.len() * 3 {
        TextEncoding::Windows1252
    } else {
        return None;
    };
    if strictness == Strictness::Sniff {
        let invalid_ok = encoding == TextEncoding::Windows1252 || utf8.invalid * 10 <= head.len();
        let control = head
            .iter()
            .filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0c | 0x1b))
            .count();
        if !invalid_ok || control * 20 > head.len() {
            return None;
        }
    }
    Some(plain(encoding))
}

const fn plain(encoding: TextEncoding) -> Detected {
    Detected { encoding, bom_len: 0 }
}

fn by_bom(head: &[u8]) -> Option<Detected> {
    let (encoding, bom_len) = match head {
        [0xEF, 0xBB, 0xBF, ..] => (TextEncoding::Utf8, 3),
        // Before UTF-16 LE, whose BOM is a prefix of this one.
        [0xFF, 0xFE, 0, 0, ..] => (TextEncoding::Utf32Le, 4),
        [0, 0, 0xFE, 0xFF, ..] => (TextEncoding::Utf32Be, 4),
        [0xFF, 0xFE, ..] => (TextEncoding::Utf16Le, 2),
        [0xFE, 0xFF, ..] => (TextEncoding::Utf16Be, 2),
        _ => return None,
    };
    Some(Detected { encoding, bom_len })
}

/// UTF-16/32 without BOM, recognised by mostly-ASCII text: a NUL in the same position(s) of
/// almost every code unit, and almost never in the character byte.
fn by_nul_pattern(window: &[u8]) -> Option<TextEncoding> {
    let quads = window.as_chunks::<4>().0;
    if quads.len() >= 2 {
        let le = quads.iter().filter(|q| q[0] != 0 && q[1..] == [0, 0, 0]).count();
        let be = quads.iter().filter(|q| q[3] != 0 && q[..3] == [0, 0, 0]).count();
        if le * 10 >= quads.len() * 9 {
            return Some(TextEncoding::Utf32Le);
        }
        if be * 10 >= quads.len() * 9 {
            return Some(TextEncoding::Utf32Be);
        }
    }
    let pairs = window.as_chunks::<2>().0;
    if pairs.len() < 2 {
        return None;
    }
    let zero_odd = pairs.iter().filter(|p| p[1] == 0).count();
    let zero_even = pairs.iter().filter(|p| p[0] == 0).count();
    let mostly = |n: usize| n * 10 >= pairs.len() * 7;
    let rarely = |n: usize| n * 20 <= pairs.len();
    if mostly(zero_odd) && rarely(zero_even) {
        Some(TextEncoding::Utf16Le)
    } else if mostly(zero_even) && rarely(zero_odd) {
        Some(TextEncoding::Utf16Be)
    } else {
        None
    }
}

fn control_ratio_ok(text: &str, strictness: Strictness) -> bool {
    if strictness == Strictness::KnownText {
        return true;
    }
    let mut total = 0usize;
    let mut control = 0usize;
    for c in text.chars() {
        total += 1;
        if c.is_control() && !matches!(c, '\t' | '\n' | '\r' | '\u{0c}' | '\u{1b}') {
            control += 1;
        }
    }
    control * 20 <= total
}

#[expect(
    clippy::naive_bytecount,
    reason = "runs once on a bounded sample; not worth a dependency"
)]
fn memchr_count(needle: u8, haystack: &[u8]) -> usize {
    haystack.iter().filter(|&&b| b == needle).count()
}

/// Counts of a UTF-8 validation pass.
struct Utf8Stats {
    /// Bytes that are not part of a valid sequence.
    invalid: usize,
    /// Valid characters of two or more bytes.
    multibyte: usize,
}

impl Utf8Stats {
    fn of(bytes: &[u8], at_eof: bool) -> Self {
        let cut = if at_eof {
            bytes.len()
        } else {
            complete_utf8_len(bytes)
        };
        let mut stats = Self {
            invalid: 0,
            multibyte: 0,
        };
        for chunk in bytes[..cut].utf8_chunks() {
            stats.invalid += chunk.invalid().len();
            // Every non-ASCII char in valid UTF-8 is a multi-byte one.
            stats.multibyte += chunk.valid().bytes().filter(|b| b & 0xC0 == 0xC0).count();
        }
        stats
    }
}

/// Length of `bytes` without a multi-byte UTF-8 sequence cut off at the end.
fn complete_utf8_len(bytes: &[u8]) -> usize {
    let len = bytes.len();
    for back in 1..=len.min(3) {
        let b = bytes[len - back];
        if b & 0xC0 == 0x80 {
            continue; // continuation byte: keep looking for the lead byte
        }
        let needed = match b {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => return len, // ASCII or an invalid lead: nothing incomplete
        };
        return if needed > back { len - back } else { len };
    }
    len
}

/// Decodes `bytes` (which start at a character boundary). Returns the text and how many bytes
/// were consumed: unless `at_eof`, a character cut off at the end is left for the next chunk.
/// Invalid sequences become U+FFFD, NUL becomes the visible U+2400.
pub(crate) fn decode(bytes: &[u8], encoding: TextEncoding, at_eof: bool) -> (String, usize) {
    let (mut text, consumed) = match encoding {
        TextEncoding::Utf8 => {
            let cut = if at_eof {
                bytes.len()
            } else {
                complete_utf8_len(bytes)
            };
            (String::from_utf8_lossy(&bytes[..cut]).into_owned(), cut)
        }
        TextEncoding::Utf16Le => decode_utf16(bytes, at_eof, u16::from_le_bytes),
        TextEncoding::Utf16Be => decode_utf16(bytes, at_eof, u16::from_be_bytes),
        TextEncoding::Utf32Le => decode_utf32(bytes, at_eof, u32::from_le_bytes),
        TextEncoding::Utf32Be => decode_utf32(bytes, at_eof, u32::from_be_bytes),
        TextEncoding::Windows1252 => (decode_windows_1252(bytes), bytes.len()),
    };
    if text.contains('\0') {
        text = text.replace('\0', "\u{2400}");
    }
    (text, consumed)
}

fn decode_utf16(bytes: &[u8], at_eof: bool, unit: fn([u8; 2]) -> u16) -> (String, usize) {
    let (pairs, odd) = bytes.as_chunks::<2>();
    let mut units: &[[u8; 2]] = pairs;
    if !at_eof && let Some((last, rest)) = units.split_last() {
        // A high surrogate needs its low half from the next chunk.
        if (0xD800..0xDC00).contains(&unit(*last)) {
            units = rest;
        }
    }
    let mut text = String::with_capacity(units.len() + units.len() / 2);
    text.extend(
        char::decode_utf16(units.iter().map(|p| unit(*p))).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)),
    );
    let mut consumed = units.len() * 2;
    if at_eof && !odd.is_empty() {
        text.push(char::REPLACEMENT_CHARACTER);
        consumed += odd.len();
    }
    (text, consumed)
}

fn decode_utf32(bytes: &[u8], at_eof: bool, unit: fn([u8; 4]) -> u32) -> (String, usize) {
    let (quads, rest) = bytes.as_chunks::<4>();
    let mut text = String::with_capacity(quads.len());
    text.extend(
        quads
            .iter()
            .map(|q| char::from_u32(unit(*q)).unwrap_or(char::REPLACEMENT_CHARACTER)),
    );
    let mut consumed = quads.len() * 4;
    if at_eof && !rest.is_empty() {
        text.push(char::REPLACEMENT_CHARACTER);
        consumed += rest.len();
    }
    (text, consumed)
}

/// Windows-1252 code points for 0x80–0x9F; the rest of the upper half is Latin-1. The five
/// unassigned positions decode to U+FFFD.
const WINDOWS_1252_HIGH: [char; 32] = [
    '€', '\u{fffd}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{fffd}', 'Ž', '\u{fffd}',
    '\u{fffd}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{fffd}', 'ž', 'Ÿ',
];

fn decode_windows_1252(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() + bytes.len() / 8);
    for &b in bytes {
        text.push(match b {
            0x80..=0x9F => WINDOWS_1252_HIGH[usize::from(b - 0x80)],
            _ => char::from(b),
        });
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str, le: bool, bom: bool) -> Vec<u8> {
        let mut out = Vec::new();
        if bom {
            out.extend(if le { [0xFF, 0xFE] } else { [0xFE, 0xFF] });
        }
        for unit in s.encode_utf16() {
            out.extend(if le {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        out
    }

    fn utf32(s: &str, le: bool, bom: bool) -> Vec<u8> {
        let mut out = Vec::new();
        if bom {
            out.extend(if le {
                [0xFF, 0xFE, 0, 0]
            } else {
                [0, 0, 0xFE, 0xFF]
            });
        }
        for c in s.chars() {
            let v = u32::from(c);
            out.extend(if le { v.to_le_bytes() } else { v.to_be_bytes() });
        }
        out
    }

    /// Detects and decodes a complete file.
    fn roundtrip(bytes: &[u8], strictness: Strictness) -> Option<(TextEncoding, String)> {
        let found = detect(bytes, true, strictness)?;
        let (text, consumed) = decode(&bytes[found.bom_len..], found.encoding, true);
        assert_eq!(
            consumed + found.bom_len,
            bytes.len(),
            "everything consumed at EOF"
        );
        Some((found.encoding, text))
    }

    const SAMPLE: &str = "Grüße € and a \"quote\"\r\nline 2\n";

    #[test]
    fn unicode_encodings_with_and_without_bom_round_trip() {
        let mut bom8 = vec![0xEF, 0xBB, 0xBF];
        bom8.extend(SAMPLE.as_bytes());
        let cases = [
            (SAMPLE.as_bytes().to_vec(), TextEncoding::Utf8),
            (bom8, TextEncoding::Utf8),
            (utf16(SAMPLE, true, true), TextEncoding::Utf16Le),
            (utf16(SAMPLE, false, true), TextEncoding::Utf16Be),
            (utf16(SAMPLE, true, false), TextEncoding::Utf16Le),
            (utf16(SAMPLE, false, false), TextEncoding::Utf16Be),
            (utf32(SAMPLE, true, true), TextEncoding::Utf32Le),
            (utf32(SAMPLE, false, true), TextEncoding::Utf32Be),
            (utf32(SAMPLE, true, false), TextEncoding::Utf32Le),
            (utf32(SAMPLE, false, false), TextEncoding::Utf32Be),
        ];
        for (i, (bytes, encoding)) in cases.into_iter().enumerate() {
            for strictness in [Strictness::Sniff, Strictness::KnownText] {
                assert_eq!(
                    roundtrip(&bytes, strictness),
                    Some((encoding, SAMPLE.to_owned())),
                    "case {i} ({encoding}, {strictness:?})"
                );
            }
        }
    }

    #[test]
    fn windows_1252_round_trips() {
        // "Grüße €" in Windows-1252: ü = FC, ß = DF, € = 80.
        let bytes = b"Gr\xfc\xdfe \x80 \x93quoted\x94\n";
        assert_eq!(
            roundtrip(bytes, Strictness::Sniff),
            Some((TextEncoding::Windows1252, "Grüße € “quoted”\n".to_owned()))
        );
        assert_eq!(
            decode(b"\x81\x8d", TextEncoding::Windows1252, true).0,
            "\u{fffd}\u{fffd}"
        );
    }

    #[test]
    fn mostly_valid_utf8_stays_utf8() {
        let mut bytes = "ÄÖÜ äöü ß — fine\nbut one stray byte: ".as_bytes().to_vec();
        bytes.push(0xFF);
        bytes.extend(b" and another: ");
        bytes.push(0xE9);
        let (encoding, text) = roundtrip(&bytes, Strictness::Sniff).expect("text");
        assert_eq!(encoding, TextEncoding::Utf8);
        assert!(text.starts_with("ÄÖÜ äöü"));
    }

    #[test]
    fn binary_is_rejected() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\x01\0\0\0\x01\0\x08\x06\0\0\0";
        let elf = b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0\x03\0\x3e\0\x01\0\0\0";
        let mut state = 0x1234_5678_u32;
        let random: Vec<u8> = (0..8192)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        for (name, bytes) in [("png", &png[..]), ("elf", &elf[..]), ("random", &random[..])] {
            for strictness in [Strictness::Sniff, Strictness::KnownText] {
                assert_eq!(detect(bytes, true, strictness), None, "{name} {strictness:?}");
            }
        }
    }

    #[test]
    fn known_text_tolerates_a_few_nuls() {
        let mut log = b"line\n".repeat(100);
        log.push(0);
        assert_eq!(detect(&log, true, Strictness::Sniff), None);
        let (encoding, text) = roundtrip(&log, Strictness::KnownText).expect("text");
        assert_eq!(encoding, TextEncoding::Utf8);
        assert!(text.ends_with('\u{2400}'), "NUL is shown as a symbol");
    }

    #[test]
    fn a_cut_multibyte_character_is_left_for_the_next_chunk() {
        let bytes = "aä€😀".as_bytes();
        for cut in 0..=bytes.len() {
            let (head, consumed) = decode(&bytes[..cut], TextEncoding::Utf8, false);
            assert!(!head.contains('\u{fffd}'), "cut {cut}: {head:?}");
            let (tail, _) = decode(&bytes[consumed..], TextEncoding::Utf8, true);
            assert_eq!(head + &tail, "aä€😀", "cut {cut}");
        }
        let units = utf16("a😀b", true, false);
        for cut in 0..=units.len() {
            let (head, consumed) = decode(&units[..cut], TextEncoding::Utf16Le, false);
            assert!(!head.contains('\u{fffd}'), "utf-16 cut {cut}: {head:?}");
            let (tail, _) = decode(&units[consumed..], TextEncoding::Utf16Le, true);
            assert_eq!(head + &tail, "a😀b", "utf-16 cut {cut}");
        }
        let quads = utf32("a😀", false, false);
        let (head, consumed) = decode(&quads[..6], TextEncoding::Utf32Be, false);
        assert_eq!((head.as_str(), consumed), ("a", 4));
    }

    #[test]
    fn a_cut_character_in_the_detection_window_is_not_corruption() {
        let mut data = vec![b'a'; 100];
        data.extend(&"€".as_bytes()[..1]);
        assert_eq!(
            detect(&data, false, Strictness::Sniff),
            Some(plain(TextEncoding::Utf8))
        );
    }
}

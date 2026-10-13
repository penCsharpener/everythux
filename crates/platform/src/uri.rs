//! `file://` URIs.

use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_encode};

/// Everything except the RFC 3986 unreserved characters (`A-Z a-z 0-9 - . _ ~`) and the path
/// separator `/`. Non-ASCII bytes are always encoded by `percent_encode`.
const ENCODE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

/// Percent-encodes an absolute path (raw bytes, so non-UTF-8 names survive) as a `file://` URI.
///
/// ```
/// use std::path::Path;
/// use everythux_platform::uri::file_uri;
///
/// assert_eq!(file_uri(Path::new("/tmp/a b#1")), "file:///tmp/a%20b%231");
/// ```
pub fn file_uri(path: &Path) -> String {
    let encoded = percent_encode(path.as_os_str().as_bytes(), ENCODE);
    let mut uri = String::with_capacity("file://".len() + path.as_os_str().len() + 8);
    uri.push_str("file://");
    uri.extend(encoded);
    uri
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use percent_encoding::percent_decode_str;
    use proptest::prelude::*;

    use super::*;

    fn uri_of(bytes: &[u8]) -> String {
        file_uri(Path::new(OsStr::from_bytes(bytes)))
    }

    #[test]
    fn plain_path_is_unchanged() {
        assert_eq!(uri_of(b"/home/a/b-c_d.e~f/G1"), "file:///home/a/b-c_d.e~f/G1");
    }

    #[test]
    fn reserved_characters_are_encoded() {
        for (raw, want) in [
            (&b"/tmp/a b"[..], "file:///tmp/a%20b"),
            (b"/tmp/50%", "file:///tmp/50%25"),
            (b"/tmp/#hash", "file:///tmp/%23hash"),
            (b"/tmp/what?", "file:///tmp/what%3F"),
            (b"/tmp/a&b=c;d+e", "file:///tmp/a%26b%3Dc%3Bd%2Be"),
            (b"/tmp/[x](y)'z'", "file:///tmp/%5Bx%5D%28y%29%27z%27"),
        ] {
            assert_eq!(uri_of(raw), want, "{}", String::from_utf8_lossy(raw));
        }
    }

    #[test]
    fn non_ascii_and_invalid_utf8_are_encoded_bytewise() {
        assert_eq!(uri_of("/tmp/\u{e4}".as_bytes()), "file:///tmp/%C3%A4");
        assert_eq!(uri_of(b"/tmp/a\xffb"), "file:///tmp/a%FFb");
    }

    #[test]
    fn root_and_empty() {
        assert_eq!(uri_of(b"/"), "file:///");
        assert_eq!(uri_of(b""), "file://");
    }

    proptest! {
        #[test]
        fn round_trips_to_the_same_bytes(raw in proptest::collection::vec(1u8..=255, 0..64)) {
            let uri = uri_of(&raw);
            let rest = uri.strip_prefix("file://").expect("scheme prefix");
            prop_assert!(rest.is_ascii());
            let decoded: Vec<u8> = percent_decode_str(rest).collect();
            prop_assert_eq!(decoded, raw);
        }

        #[test]
        fn output_only_has_safe_characters(raw in proptest::collection::vec(1u8..=255, 0..64)) {
            let uri = uri_of(&raw);
            let rest = uri.strip_prefix("file://").expect("scheme prefix");
            for c in rest.chars() {
                prop_assert!(
                    c.is_ascii_alphanumeric() || "-._~/%".contains(c),
                    "unexpected {c:?} in {uri}"
                );
            }
        }
    }
}

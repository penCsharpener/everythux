//! Tokenizer: splits the search box text into pieces (terms and filters) and `|` separators.

use crate::query::QueryError;

/// The filters understood by the parser. A `name:` prefix is only a filter when `name` is one of
/// these; anything else (`12:30`, `c:\x`) is an ordinary term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FilterKind {
    Ext,
    Size,
    Dm,
    Path,
    File,
    Folder,
    Regex,
    Case,
    Vol,
}

impl FilterKind {
    pub(super) fn from_name(name: &str) -> Option<Self> {
        const NAMES: [(&str, FilterKind); 11] = [
            ("ext", FilterKind::Ext),
            ("extension", FilterKind::Ext),
            ("size", FilterKind::Size),
            ("dm", FilterKind::Dm),
            ("datemodified", FilterKind::Dm),
            ("path", FilterKind::Path),
            ("file", FilterKind::File),
            ("folder", FilterKind::Folder),
            ("regex", FilterKind::Regex),
            ("case", FilterKind::Case),
            ("vol", FilterKind::Vol),
        ];
        NAMES
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|&(_, k)| k)
    }

    /// The canonical spelling.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Ext => "ext",
            Self::Size => "size",
            Self::Dm => "dm",
            Self::Path => "path",
            Self::File => "file",
            Self::Folder => "folder",
            Self::Regex => "regex",
            Self::Case => "case",
            Self::Vol => "vol",
        }
    }
}

/// One whitespace-delimited word with its quotes removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Piece {
    /// Leading `!` seen an odd number of times.
    pub neg: bool,
    /// Text with quotes removed.
    pub text: String,
    /// Byte offset in `text` where the first quote opened (so a `name:` prefix before it is a real
    /// prefix, while `"ext:pdf"` is a phrase).
    quote_start: Option<usize>,
}

impl Piece {
    fn new() -> Self {
        Self {
            neg: false,
            text: String::new(),
            quote_start: None,
        }
    }

    /// The filter and its value, when the piece starts with `name:` for a known filter name.
    pub(super) fn filter(&self) -> Option<(FilterKind, &str)> {
        let idx = self.text.find(':')?;
        if idx == 0 || self.quote_start.is_some_and(|q| idx >= q) {
            return None;
        }
        let name = &self.text[..idx];
        if !name.bytes().all(|b| b.is_ascii_alphabetic()) {
            return None;
        }
        Some((FilterKind::from_name(name)?, &self.text[idx + 1..]))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Item {
    Piece(Piece),
    Pipe,
}

/// Splits `input` into pieces and pipes.
///
/// `name:a|b` is shorthand for `name:a|name:b`: a word that directly follows a `|` that directly
/// follows a filter with a value inherits that filter (`dm:today|yesterday`, `ext:jpg|png`).
pub(super) fn lex(input: &str) -> Result<Vec<Item>, QueryError> {
    let mut items = Vec::new();
    let mut cur: Option<Piece> = None;
    let mut in_quote = false;
    let mut inherit: Option<FilterKind> = None;

    for ch in input.chars() {
        if in_quote {
            if ch == '"' {
                in_quote = false;
            } else if let Some(p) = cur.as_mut() {
                p.text.push(ch);
            }
            continue;
        }
        match ch {
            '"' => {
                in_quote = true;
                let p = cur.get_or_insert_with(Piece::new);
                p.quote_start.get_or_insert(p.text.len());
            }
            '|' => {
                let adjacent = cur.is_some();
                let filter = flush(&mut cur, &mut items, &mut inherit);
                items.push(Item::Pipe);
                inherit = if adjacent { filter } else { None };
            }
            c if c.is_whitespace() => {
                flush(&mut cur, &mut items, &mut inherit);
                inherit = None;
            }
            '!' if cur
                .as_ref()
                .is_none_or(|p| p.text.is_empty() && p.quote_start.is_none()) =>
            {
                let p = cur.get_or_insert_with(Piece::new);
                p.neg = !p.neg;
            }
            c => cur.get_or_insert_with(Piece::new).text.push(c),
        }
    }
    if in_quote {
        return Err(QueryError::UnterminatedQuote);
    }
    flush(&mut cur, &mut items, &mut inherit);
    Ok(items)
}

/// Pushes the pending piece (dropping empty ones) and returns its filter when it has a value.
fn flush(
    cur: &mut Option<Piece>,
    items: &mut Vec<Item>,
    inherit: &mut Option<FilterKind>,
) -> Option<FilterKind> {
    let mut piece = cur.take()?;
    let inherited = inherit.take();
    if piece.text.is_empty() {
        return None;
    }
    if let Some(kind) = inherited
        && piece.filter().is_none()
    {
        let prefix = format!("{}:", kind.name());
        piece.text.insert_str(0, &prefix);
        if let Some(q) = piece.quote_start.as_mut() {
            *q += prefix.len();
        }
    }
    let filter = piece.filter().filter(|(_, v)| !v.is_empty()).map(|(k, _)| k);
    items.push(Item::Piece(piece));
    filter
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(input: &str) -> Vec<String> {
        lex(input)
            .unwrap()
            .into_iter()
            .map(|i| match i {
                Item::Piece(p) => format!("{}{}", if p.neg { "!" } else { "" }, p.text),
                Item::Pipe => "|".to_owned(),
            })
            .collect()
    }

    #[test]
    fn splits_words_pipes_and_negation() {
        assert_eq!(texts("foo  bar|baz"), ["foo", "bar", "|", "baz"]);
        assert_eq!(texts("a | b"), ["a", "|", "b"]);
        assert_eq!(texts("!foo !!bar"), ["!foo", "bar"]);
        assert_eq!(texts("a!b"), ["a!b"]);
        assert_eq!(texts("!"), Vec::<String>::new());
    }

    #[test]
    fn quotes_keep_spaces_and_pipes() {
        assert_eq!(texts(r#""a b|c" d"#), ["a b|c", "d"]);
        assert_eq!(texts(r#"foo"bar baz"qux"#), ["foobar bazqux"]);
        assert_eq!(texts(r#"!"a b""#), ["!a b"]);
        assert_eq!(texts(r#""""#), Vec::<String>::new());
    }

    #[test]
    fn unterminated_quote_is_an_error() {
        assert_eq!(lex(r#"foo "bar"#), Err(QueryError::UnterminatedQuote));
    }

    #[test]
    fn filter_prefix_requires_known_name_outside_quotes() {
        let first = |s: &str| match lex(s).unwrap().remove(0) {
            Item::Piece(p) => p,
            Item::Pipe => unreachable!(),
        };
        assert_eq!(
            first("ext:pdf").filter().map(|(k, v)| (k, v.to_owned())),
            Some((FilterKind::Ext, "pdf".to_owned()))
        );
        assert!(first("12:30").filter().is_none());
        assert!(first("c:x").filter().is_none());
        assert!(first(r#""ext:pdf""#).filter().is_none());
        assert!(first("EXT:pdf").filter().is_some());
    }

    #[test]
    fn filter_values_continue_after_a_pipe() {
        assert_eq!(texts("dm:today|yesterday"), ["dm:today", "|", "dm:yesterday"]);
        assert_eq!(texts("ext:a|b|c"), ["ext:a", "|", "ext:b", "|", "ext:c"]);
        assert_eq!(texts("ext:a|path:x"), ["ext:a", "|", "path:x"]);
        assert_eq!(texts("ext:a | b"), ["ext:a", "|", "b"]);
        assert_eq!(texts("foo|bar"), ["foo", "|", "bar"]);
    }
}

//! Query language (Everything-style).
//!
//! - whitespace = AND, `|` = OR (binds tighter than AND), leading `!` = NOT
//! - `"quoted phrase"` keeps spaces; `*` and `?` are wildcards (then the term must match the whole
//!   name, like Everything); otherwise a term matches as a substring
//! - filters: `ext:pdf;docx`, `size:>100mb`, `size:1mb..10mb`, `dm:today|yesterday|thisweek|lastweek|
//!   thismonth|thisyear|2026-01-01|2026-01-01..2026-03-01`, `path:<substr>`, `file:`, `folder:`,
//!   `regex:<re>`, `case:<term>`, `vol:<mount-point substring>`
//! - `<` `>` group words into one operand: `<a b>|<c d>` is `(a AND b) OR (c AND d)`; groups nest and
//!   `!<a b>` negates one. `<` opens a group only at the start of a word and `>` closes it only at the
//!   end of a word, so `size:>1mb` and `size:<1kb` keep their comparisons; a group left open runs to
//!   the end of the query
//! - an empty query matches everything
//!
//! # Details
//!
//! - `a b|c` is `a AND (b OR c)`; `a | b` (spaces around the pipe) is the same as `a|b`. After a
//!   filter, `|` continues the filter's values: `ext:jpg|png`, `dm:today|yesterday`,
//!   `size:<1kb|>1gb` are shorthand for repeating the filter.
//! - A `name:` prefix is a filter only if `name` is a known filter (`ext`, `size`, `dm`, `path`, `file`,
//!   `folder`, `regex`, `case`, `vol`); anything else (`12:30`) is a plain term.
//! - A filter with an empty value (`path:` while still typing) is ignored. Bare `file:` / `folder:`
//!   keep only files / folders; with a value they also require the term: `folder:src`. Bare `case:`
//!   makes the whole query case-sensitive; `case:Foo` only that term.
//! - Sizes accept `b`, `kb`, `mb`, `gb`, `tb` (also `kib`, ...; all powers of 1024, like Everything) and
//!   decimals (`1.5gb`). Dates are interpreted in the local timezone; weeks start on Monday.
//! - [`QueryOptions::match_path`] and `path:` match a term against the full path (`dir/name`).
//!   [`QueryOptions::regex`] treats the entire input as one regular expression.

mod filters;
mod lex;
mod term;

use chrono::{DateTime, Local, NaiveDate, NaiveTime, TimeDelta, TimeZone};
use everythux_core::config::SearchConfig;
use everythux_core::text::fold;

use self::filters::{Calendar, parse_date, parse_ext, parse_size};
use self::lex::{FilterKind, Item, Piece, lex};
pub(crate) use self::term::{Glob, Matcher, Scope, Term};

/// Global switches from the UI / config.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueryOptions {
    /// Case-sensitive matching for all terms.
    pub case_sensitive: bool,
    /// Match terms against the full path instead of the name.
    pub match_path: bool,
    /// Treat the whole input as one regular expression.
    pub regex: bool,
}

impl From<&SearchConfig> for QueryOptions {
    fn from(c: &SearchConfig) -> Self {
        Self {
            case_sensitive: c.case_sensitive,
            match_path: c.match_path,
            regex: c.regex,
        }
    }
}

/// A parsed query AST node. Leaves are compiled (finders, regexes, numeric bounds) so matching
/// needs no further parsing.
#[derive(Debug, Clone)]
pub(crate) enum Expr {
    /// Matches everything.
    True,
    /// Matches nothing.
    False,
    /// All children match.
    And(Vec<Expr>),
    /// Any child matches.
    Or(Vec<Expr>),
    /// The child does not match.
    Not(Box<Expr>),
    /// Text term.
    Term(Term),
    /// Extension in the set (lowercase, no dot).
    Ext(Vec<Box<str>>),
    /// Size within `lo..=hi` bytes.
    Size { lo: u64, hi: u64 },
    /// Modification time within `lo..hi` ns since the epoch.
    Mtime { lo: i64, hi: i64 },
    /// Folders only (`true`) or everything except folders (`false`).
    Folder(bool),
    /// Volume whose mount point or label contains `needle`.
    Volume { needle: Vec<u8>, fold: bool },
}

impl Expr {
    fn and(mut children: Vec<Self>) -> Self {
        let mut flat = Vec::with_capacity(children.len());
        for c in children.drain(..) {
            match c {
                Self::True => {}
                Self::False => return Self::False,
                Self::And(inner) => flat.extend(inner),
                other => flat.push(other),
            }
        }
        match flat.len() {
            0 => Self::True,
            1 => flat.remove(0),
            _ => Self::And(flat),
        }
    }

    fn or(mut children: Vec<Self>) -> Self {
        let mut flat = Vec::with_capacity(children.len());
        for c in children.drain(..) {
            match c {
                Self::False => {}
                Self::True => return Self::True,
                Self::Or(inner) => flat.extend(inner),
                other => flat.push(other),
            }
        }
        match flat.len() {
            0 => Self::False,
            1 => flat.remove(0),
            _ => Self::Or(flat),
        }
    }

    fn not(child: Self) -> Self {
        match child {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Not(inner) => *inner,
            Self::Folder(dir) => Self::Folder(!dir),
            other => Self::Not(Box::new(other)),
        }
    }
}

/// A parsed query (opaque; produced by [`parse`], consumed by [`crate::SearchIndex::search`]).
#[derive(Debug, Clone)]
pub struct Query {
    expr: Expr,
    options: QueryOptions,
}

impl Query {
    /// A query that matches every entry.
    pub fn match_all() -> Self {
        Self {
            expr: Expr::True,
            options: QueryOptions::default(),
        }
    }

    /// Whether the query matches every entry (empty input).
    pub fn is_match_all(&self) -> bool {
        matches!(self.expr, Expr::True)
    }

    /// The options the query was parsed with.
    pub fn options(&self) -> QueryOptions {
        self.options
    }

    /// The AND of `queries`, as if each were one search bar.
    ///
    /// Nested ANDs are flattened, so the engine orders the combined clauses by cost exactly as it
    /// would for a single query holding the same terms. An empty list matches everything. The
    /// result carries the options of the first query (the switches are global, only a bare
    /// `case:` can differ per bar, and it is already compiled into that bar's terms).
    pub fn all(queries: Vec<Query>) -> Self {
        let options = queries.first().map(Query::options).unwrap_or_default();
        Self {
            expr: Expr::and(queries.into_iter().map(|q| q.expr).collect()),
            options,
        }
    }

    pub(crate) fn expr(&self) -> &Expr {
        &self.expr
    }
}

/// A parse error in one of several search bars (see [`parse_many`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("search bar {}: {error}", bar + 1)]
pub struct BarQueryError {
    /// Index of the failing text in the slice passed to [`parse_many`] (0 = first bar).
    pub bar: usize,
    /// What is wrong with it.
    #[source]
    pub error: QueryError,
}

/// A query that can't be parsed (shown under the search box; the previous results stay).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum QueryError {
    /// Invalid regex.
    #[error("invalid regular expression: {0}")]
    Regex(String),
    /// Unknown `name:` filter or invalid filter value.
    #[error("invalid filter {filter:?}: {message}")]
    Filter {
        /// The filter text.
        filter: String,
        /// What's wrong.
        message: String,
    },
    /// Unbalanced quotes.
    #[error("unterminated quote")]
    UnterminatedQuote,
}

/// Parses search box text, using the local clock for `dm:` filters.
pub fn parse(input: &str, options: QueryOptions) -> Result<Query, QueryError> {
    parse_at(input, options, &Local::now())
}

/// Like [`parse`], but with an explicit "now" (and thereby timezone) for `dm:` filters, so tests
/// don't depend on the wall clock.
pub fn parse_at<Tz: TimeZone>(
    input: &str,
    options: QueryOptions,
    now: &DateTime<Tz>,
) -> Result<Query, QueryError> {
    let tz = now.timezone();
    let midnight_ns = |date: NaiveDate| {
        let local = date.and_time(NaiveTime::MIN);
        let secs = tz
            .from_local_datetime(&local)
            .earliest()
            // midnight doesn't exist on this day (DST gap): use the first instant after the gap
            .or_else(|| tz.from_local_datetime(&(local + TimeDelta::hours(1))).earliest())
            .map_or_else(|| local.and_utc().timestamp(), |dt| dt.timestamp());
        secs.saturating_mul(1_000_000_000)
    };
    let cal = Calendar {
        today: now.date_naive(),
        midnight_ns: &midnight_ns,
    };
    Parser { options, cal: &cal }.parse(input)
}

/// Parses each text on its own and combines the results with AND ([`Query::all`]).
///
/// This is what several search bars mean: an unterminated quote or a trailing `|` in one text can't
/// leak into another, unlike concatenating them. Empty texts match everything and so change
/// nothing.
///
/// # Errors
///
/// The first text that fails to parse, with its index.
pub fn parse_many<S: AsRef<str>>(texts: &[S], options: QueryOptions) -> Result<Query, BarQueryError> {
    parse_many_at(texts, options, &Local::now())
}

/// Like [`parse_many`], with an explicit "now" for `dm:` filters.
///
/// # Errors
///
/// The first text that fails to parse, with its index.
pub fn parse_many_at<S: AsRef<str>, Tz: TimeZone>(
    texts: &[S],
    options: QueryOptions,
    now: &DateTime<Tz>,
) -> Result<Query, BarQueryError> {
    let queries = texts
        .iter()
        .enumerate()
        .map(|(bar, text)| {
            parse_at(text.as_ref(), options, now).map_err(|error| BarQueryError { bar, error })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if queries.is_empty() {
        // Keep the switches even though no text was given.
        return Ok(Query {
            expr: Expr::True,
            options,
        });
    }
    Ok(Query::all(queries))
}

struct Parser<'a> {
    options: QueryOptions,
    cal: &'a Calendar<'a>,
}

impl Parser<'_> {
    fn parse(mut self, input: &str) -> Result<Query, QueryError> {
        let options = self.options;
        let scope = if options.match_path {
            Scope::Path
        } else {
            Scope::Name
        };
        if input.trim().is_empty() {
            return Ok(Query {
                expr: Expr::True,
                options,
            });
        }
        if options.regex {
            let term = Term::regex(input, options.case_sensitive, scope)?;
            return Ok(Query {
                expr: Expr::Term(term),
                options,
            });
        }

        let items = lex(input)?;
        // A bare `case:` anywhere switches the whole query to case-sensitive matching.
        let bare_case = items.iter().any(|item| {
            matches!(item, Item::Piece(p) if !p.neg && matches!(p.filter(), Some((FilterKind::Case, v)) if v.is_empty()))
        });
        if bare_case {
            self.options.case_sensitive = true;
        }

        let expr = self.group(&mut items.into_iter())?.unwrap_or(Expr::True);
        Ok(Query {
            expr,
            options: self.options,
        })
    }

    /// Parses items up to the `>` that closes this group (or the end of the input) into one
    /// expression: AND of OR-runs, where `|` joins the operands next to it. `None` when the
    /// group holds no operand (`<>`), so it can be skipped like an empty filter.
    fn group(&self, items: &mut std::vec::IntoIter<Item>) -> Result<Option<Expr>, QueryError> {
        let mut operands = Vec::new();
        let mut group: Vec<Expr> = Vec::new();
        let mut joining = false;
        while let Some(item) = items.next() {
            let expr = match item {
                Item::Close => break,
                Item::Pipe => {
                    joining = !group.is_empty();
                    continue;
                }
                Item::Piece(piece) => self.piece(&piece)?,
                Item::Open { neg } => self
                    .group(items)?
                    .map(|inner| if neg { Expr::not(inner) } else { inner }),
            };
            let Some(expr) = expr else { continue };
            if !joining && !group.is_empty() {
                operands.push(Expr::or(std::mem::take(&mut group)));
            }
            group.push(expr);
            joining = false;
        }
        if !group.is_empty() {
            operands.push(Expr::or(group));
        }
        Ok((!operands.is_empty()).then(|| Expr::and(operands)))
    }

    /// Builds the expression for one word; `None` for a no-op (empty filter value).
    fn piece(&self, piece: &Piece) -> Result<Option<Expr>, QueryError> {
        let expr = match piece.filter() {
            Some((kind, value)) => match self.filter(kind, value, &piece.text)? {
                Some(expr) => expr,
                None => return Ok(None),
            },
            None => self.term_expr(&piece.text, self.options.case_sensitive)?,
        };
        Ok(Some(if piece.neg { Expr::not(expr) } else { expr }))
    }

    fn filter(&self, kind: FilterKind, value: &str, text: &str) -> Result<Option<Expr>, QueryError> {
        let wants_value = !matches!(kind, FilterKind::File | FilterKind::Folder);
        if wants_value && value.trim().is_empty() {
            return Ok(None);
        }
        let bad = |message: String| QueryError::Filter {
            filter: text.to_owned(),
            message,
        };
        let name_scope = if self.options.match_path {
            Scope::Path
        } else {
            Scope::Name
        };
        let sensitive = self.options.case_sensitive;
        Ok(Some(match kind {
            FilterKind::Ext => {
                let exts = parse_ext(value);
                if exts.is_empty() {
                    return Ok(None);
                }
                Expr::Ext(exts)
            }
            FilterKind::Size => match parse_size(value).map_err(bad)? {
                Some((lo, hi)) => Expr::Size { lo, hi },
                None => Expr::False,
            },
            FilterKind::Dm => match parse_date(value, self.cal).map_err(bad)? {
                Some((lo, hi)) => Expr::Mtime { lo, hi },
                None => Expr::False,
            },
            FilterKind::Path => Term::text(value, sensitive, Scope::Path)?.map_or(Expr::True, Expr::Term),
            FilterKind::File | FilterKind::Folder => {
                let kind_expr = Expr::Folder(kind == FilterKind::Folder);
                if value.trim().is_empty() {
                    kind_expr
                } else {
                    Expr::and(vec![kind_expr, self.term_expr(value, sensitive)?])
                }
            }
            FilterKind::Regex => Expr::Term(Term::regex(value, sensitive, name_scope)?),
            FilterKind::Case => self.term_expr(value, true)?,
            FilterKind::Vol => Expr::Volume {
                needle: if sensitive {
                    value.as_bytes().to_vec()
                } else {
                    fold(value.as_bytes()).into_bytes()
                },
                fold: !sensitive,
            },
        }))
    }

    /// A plain word or phrase; wildcard-only terms match everything.
    fn term_expr(&self, text: &str, case_sensitive: bool) -> Result<Expr, QueryError> {
        let scope = if self.options.match_path {
            Scope::Path
        } else {
            Scope::Name
        };
        Ok(Term::text(text, case_sensitive, scope)?.map_or(Expr::True, Expr::Term))
    }
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    fn at(input: &str) -> Result<Query, QueryError> {
        let now = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
            .unwrap();
        parse_at(input, QueryOptions::default(), &now)
    }

    /// Compact rendering of the AST for assertions.
    fn show(e: &Expr) -> String {
        match e {
            Expr::True => "T".into(),
            Expr::False => "F".into(),
            Expr::And(c) => format!("and({})", c.iter().map(show).collect::<Vec<_>>().join(",")),
            Expr::Or(c) => format!("or({})", c.iter().map(show).collect::<Vec<_>>().join(",")),
            Expr::Not(c) => format!("not({})", show(c)),
            Expr::Term(t) => match &t.matcher {
                Matcher::Sub(f) => format!("sub:{}", String::from_utf8_lossy(f.needle())),
                Matcher::Glob(_) => "glob".into(),
                Matcher::Regex(_) => "re".into(),
            },
            Expr::Ext(e) => format!("ext:{}", e.join(";")),
            Expr::Size { lo, hi } => format!("size:{lo}..{hi}"),
            Expr::Mtime { .. } => "dm".into(),
            Expr::Folder(d) => format!("folder:{d}"),
            Expr::Volume { needle, .. } => format!("vol:{}", String::from_utf8_lossy(needle)),
        }
    }

    fn ast(input: &str) -> String {
        show(at(input).unwrap().expr())
    }

    #[test]
    fn empty_input_matches_all() {
        assert!(at("").unwrap().is_match_all());
        assert!(at("   ").unwrap().is_match_all());
        assert!(at("*").unwrap().is_match_all());
        assert!(at("path:").unwrap().is_match_all());
        assert!(!at("a").unwrap().is_match_all());
    }

    #[test]
    fn groups_combine_and_terms_into_one_operand() {
        assert_eq!(ast("<a b>|<c d>"), "or(and(sub:a,sub:b),and(sub:c,sub:d))");
        assert_eq!(ast("<a|b> c"), "and(or(sub:a,sub:b),sub:c)");
        assert_eq!(ast("x <a b>|c"), "and(sub:x,or(and(sub:a,sub:b),sub:c))");
        assert_eq!(ast("!<a b>"), "not(and(sub:a,sub:b))");
        assert_eq!(ast("<a <b|c> d>"), "and(sub:a,or(sub:b,sub:c),sub:d)");
        assert_eq!(ast("<a b"), "and(sub:a,sub:b)", "an open group runs to the end");
        assert!(at("<>").unwrap().is_match_all());
        assert_eq!(
            ast("<size:>1kb ext:pdf>|<ext:txt>"),
            "or(and(size:1025..18446744073709551615,ext:pdf),ext:txt)"
        );
    }

    #[test]
    fn precedence_or_binds_tighter_than_and() {
        assert_eq!(ast("a b"), "and(sub:a,sub:b)");
        assert_eq!(ast("a|b"), "or(sub:a,sub:b)");
        assert_eq!(ast("a b|c"), "and(sub:a,or(sub:b,sub:c))");
        assert_eq!(ast("a | b c"), "and(or(sub:a,sub:b),sub:c)");
        assert_eq!(ast("a|b|c d"), "and(or(sub:a,sub:b,sub:c),sub:d)");
        assert_eq!(ast("a| b"), "or(sub:a,sub:b)");
        assert_eq!(ast("|a|"), "sub:a");
    }

    #[test]
    fn negation() {
        assert_eq!(ast("!a"), "not(sub:a)");
        assert_eq!(ast("!!a"), "sub:a");
        assert_eq!(ast("a !b|c"), "and(sub:a,or(not(sub:b),sub:c))");
        assert_eq!(ast("!folder:"), "folder:false");
        assert_eq!(ast("!ext:pdf"), "not(ext:pdf)");
    }

    #[test]
    fn terms_are_folded_unless_case_sensitive() {
        assert_eq!(ast("FoO"), "sub:foo");
        let q = parse(
            "FoO",
            QueryOptions {
                case_sensitive: true,
                ..QueryOptions::default()
            },
        )
        .unwrap();
        assert_eq!(show(q.expr()), "sub:FoO");
        assert_eq!(ast("case:FoO"), "sub:FoO");
        assert_eq!(ast("case: FoO bar"), "and(sub:FoO,sub:bar)");
    }

    #[test]
    fn phrases() {
        assert_eq!(ast(r#""my docs" x"#), "and(sub:my docs,sub:x)");
        assert_eq!(ast(r#""ext:pdf""#), "sub:ext:pdf");
    }

    #[test]
    fn wildcards() {
        assert_eq!(ast("*.txt"), "glob");
        assert_eq!(ast("a?c"), "glob");
        assert_eq!(ast("*abc*"), "sub:abc");
    }

    #[test]
    fn filters_parse() {
        assert_eq!(ast("ext:PDF;docx"), "ext:pdf;docx");
        assert_eq!(ast("ext:jpg|png"), "or(ext:jpg,ext:png)");
        assert_eq!(ast("size:>1kb"), "size:1025..18446744073709551615");
        assert_eq!(ast("size:<0"), "F");
        assert_eq!(ast("dm:today"), "dm");
        assert_eq!(ast("dm:today|yesterday"), "or(dm,dm)");
        assert_eq!(ast("file:"), "folder:false");
        assert_eq!(ast("folder:"), "folder:true");
        assert_eq!(ast("folder:src"), "and(folder:true,sub:src)");
        assert_eq!(ast("vol:Home"), "vol:home");
        assert_eq!(ast("regex:^a.*z$"), "re");
        assert_eq!(ast("path:Src/Lib"), "sub:src/lib");
    }

    #[test]
    fn unknown_prefixes_are_plain_terms() {
        assert_eq!(ast("12:30"), "sub:12:30");
        assert_eq!(ast("foo:bar"), "sub:foo:bar");
        assert_eq!(ast("c:\\x"), "sub:c:\\x");
    }

    #[test]
    fn errors() {
        assert!(matches!(at("\"abc"), Err(QueryError::UnterminatedQuote)));
        assert!(matches!(at("size:>abc"), Err(QueryError::Filter { .. })));
        assert!(matches!(at("dm:someday"), Err(QueryError::Filter { .. })));
        assert!(matches!(at("regex:("), Err(QueryError::Regex(_))));
        let opts = QueryOptions {
            regex: true,
            ..QueryOptions::default()
        };
        assert!(matches!(parse("(", opts), Err(QueryError::Regex(_))));
    }

    fn many(texts: &[&str]) -> Result<Query, BarQueryError> {
        let now = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
            .unwrap();
        parse_many_at(texts, QueryOptions::default(), &now)
    }

    #[test]
    fn all_of_nothing_matches_everything() {
        assert!(Query::all(Vec::new()).is_match_all());
        assert!(many(&[]).unwrap().is_match_all());
        assert!(many(&["", "  ", "*"]).unwrap().is_match_all());
    }

    #[test]
    fn a_single_bar_equals_a_plain_parse() {
        for text in ["a", "a b|c", "!x ext:pdf", "dm:today size:>1kb", ""] {
            assert_eq!(show(many(&[text]).unwrap().expr()), ast(text), "{text}");
        }
    }

    #[test]
    fn bars_are_flattened_into_one_and() {
        assert_eq!(show(many(&["a", "b"]).unwrap().expr()), "and(sub:a,sub:b)");
        assert_eq!(
            show(many(&["a b|c", "", "ext:pdf"]).unwrap().expr()),
            "and(sub:a,or(sub:b,sub:c),ext:pdf)"
        );
        assert_eq!(show(many(&["a", "size:<0"]).unwrap().expr()), "F");
    }

    #[test]
    fn bars_do_not_leak_into_each_other() {
        // Concatenated, `a|` would join `b` into an OR; as bars it is a lone `a`.
        assert_eq!(show(many(&["a|", "b"]).unwrap().expr()), "and(sub:a,sub:b)");
        // A bare `case:` only affects its own bar.
        assert_eq!(
            show(many(&["case: Foo", "Bar"]).unwrap().expr()),
            "and(sub:Foo,sub:bar)"
        );
    }

    #[test]
    fn the_failing_bar_is_reported() {
        let err = many(&["ok", "\"open", "size:>abc"]).unwrap_err();
        assert_eq!(err.bar, 1);
        assert_eq!(err.error, QueryError::UnterminatedQuote);
        assert_eq!(err.to_string(), "search bar 2: unterminated quote");
        assert_eq!(many(&["", "x", "regex:("]).unwrap_err().bar, 2);
    }

    #[test]
    fn regex_mode_compiles_each_bar_on_its_own() {
        let opts = QueryOptions {
            regex: true,
            ..QueryOptions::default()
        };
        let q = parse_many(&["^a", "b$"], opts).unwrap();
        assert_eq!(show(q.expr()), "and(re,re)");
        assert!(q.options().regex);
        assert_eq!(parse_many(&["a", "("], opts).unwrap_err().bar, 1);
    }

    #[test]
    fn regex_option_takes_the_whole_input() {
        let opts = QueryOptions {
            regex: true,
            ..QueryOptions::default()
        };
        assert_eq!(show(parse("a b|c", opts).unwrap().expr()), "re");
    }
}

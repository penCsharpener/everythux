//! Query language (Everything-style).
//!
//! - whitespace = AND, `|` = OR (binds tighter than AND), leading `!` = NOT
//! - `"quoted phrase"` keeps spaces; `*` and `?` are wildcards (then the term must match the whole
//!   name, like Everything); otherwise a term matches as a substring
//! - filters: `ext:pdf;docx`, `size:>100mb`, `size:1mb..10mb`, `dm:today|yesterday|thisweek|lastweek|
//!   thismonth|thisyear|2026-01-01|2026-01-01..2026-03-01`, `path:<substr>`, `file:`, `folder:`,
//!   `regex:<re>`, `case:<term>`, `vol:<mount-point substring>`
//! - an empty query matches everything

use everythux_core::config::SearchConfig;

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

/// A parsed query (opaque; produced by [`parse`], consumed by [`crate::SearchIndex::search`]).
#[derive(Debug, Clone)]
pub struct Query {
    _private: (),
}

impl Query {
    /// Whether the query matches every entry (empty input).
    pub fn is_match_all(&self) -> bool {
        todo!()
    }
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

/// Parses search box text.
pub fn parse(input: &str, options: QueryOptions) -> Result<Query, QueryError> {
    let _ = (input, options);
    todo!("Phase 3")
}

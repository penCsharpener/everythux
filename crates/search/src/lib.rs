//! Search for everythux (ROADMAP Phases 3 and 10).
//!
//! - [`query`]: parses the search box text (`foo bar|baz !qux "a b" ext:pdf;doc size:>1mb dm:today`).
//! - [`engine`]: [`SearchIndex`], a parallel substring/wildcard/regex matcher over an
//!   [`everythux_core::snapshot::IndexSnapshot`] with pre-sorted permutations per column.
//! - [`service`]: [`SearchService`], a background thread that runs the latest query, cancels stale
//!   ones and swaps snapshots.
//!
//! - `synthetic` (hidden): seeded snapshot generator and builder for tests and benchmarks.
//!
//! The re-exported items are the contract used by `everythux-app` and `everythux-cli`.

#![forbid(unsafe_code)]

pub mod engine;
mod matcher;
pub mod query;
pub mod service;
#[doc(hidden)]
pub mod synthetic;

pub use engine::{SearchIndex, Sort};
pub use query::{BarQueryError, Query, QueryError, QueryOptions, parse, parse_at, parse_many, parse_many_at};
pub use service::{SearchError, SearchRequest, SearchResults, SearchService};

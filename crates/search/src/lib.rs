//! Search for everythux (ROADMAP Phases 3 and 10).
//!
//! - [`query`]: parses the search box text (`foo bar|baz !qux "a b" ext:pdf;doc size:>1mb dm:today`).
//! - [`engine`]: [`SearchIndex`], a parallel substring/wildcard/regex matcher over an
//!   [`everythux_core::snapshot::IndexSnapshot`] with pre-sorted permutations per column.
//! - [`service`]: [`SearchService`], a background thread that runs the latest query, cancels stale
//!   ones and swaps snapshots.
//!
//! The re-exported items are the contract used by `everythux-app` and `everythux-cli`.

pub mod engine;
pub mod query;
pub mod service;

pub use engine::{SearchIndex, Sort};
pub use query::{Query, QueryError, QueryOptions, parse};
pub use service::{SearchRequest, SearchResults, SearchService};

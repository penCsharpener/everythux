//! Shared domain types for everythux.
//!
//! This crate is the contract between the indexer (`everythux-index`), the search engine
//! (`everythux-search`), the desktop integration (`everythux-platform`) and the UI. It has no
//! dependency on any other workspace crate.
//!
//! - [`ids`] / [`model`]: identifiers and plain data types (volumes, entry kinds).
//! - [`snapshot`]: the struct-of-arrays [`snapshot::IndexSnapshot`] that the indexer produces and the
//!   search engine consumes.
//! - [`text`]: name folding shared by indexing and querying.
//! - [`config`]: the JSON configuration (`config.json`).
//! - [`exclusion`]: exclusion rules and their compiled matcher.
//! - [`paths`]: XDG locations of config, database and logs.

pub mod config;
pub mod exclusion;
pub mod history;
pub mod ids;
pub mod model;
pub mod paths;
pub mod snapshot;
pub mod text;

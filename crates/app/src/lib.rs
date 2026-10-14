//! Everythux desktop UI (eframe/egui).
//!
//! The binary `everythux` only calls [`run`]. Everything else is crate-private:
//!
//! - `app`: the UI state machine (search, selection, commands, popups), free of widgets.
//! - `backend`: the trait through which the UI reaches indexer, search, preview, desktop
//!   integration and disk, with the real implementation and a test fake.
//! - `ui`: egui rendering (menus, table, status bar, windows).
//! - `exclusions`, `volumes`, `properties`, `preview`: the feature state machines.

#![forbid(unsafe_code)]

mod app;
mod backend;
mod exclusions;
mod format;
mod keys;
mod model;
mod preview;
mod properties;
mod rename;
mod runtime;
mod selection;
mod status;
mod tasks;
mod text_doc;
mod ui;
mod volumes;

#[cfg(test)]
mod testkit;

pub use runtime::run;

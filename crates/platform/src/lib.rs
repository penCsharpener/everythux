//! Desktop integration for everythux. The only crate that talks to D-Bus or spawns desktop helpers.
//!
//! - [`file_manager`]: show a file selected in its folder, show folder properties, open with the
//!   default application. Fallback chain: `org.freedesktop.FileManager1` → portal → `xdg-open`.
//! - [`uri`]: `file://` URIs from raw (possibly non-UTF-8) paths.
//! - [`mime`]: human-readable file type names for the Type column.
//! - [`udisks`]: block devices (including unmounted ext4 partitions) and mounting via UDisks2.

mod error;
pub mod file_manager;
pub mod fileops;
pub mod mime;
pub mod udisks;
pub mod uri;

pub use error::PlatformError;

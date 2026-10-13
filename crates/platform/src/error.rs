/// Errors from desktop integration.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlatformError {
    /// D-Bus call failed.
    #[error("D-Bus call failed: {0}")]
    DBus(String),
    /// Launching a helper program failed.
    #[error("cannot run {program}")]
    Spawn {
        /// Program name.
        program: String,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// No mechanism is available on this desktop.
    #[error("not supported on this desktop: {0}")]
    Unsupported(&'static str),
}

impl PlatformError {
    /// Wraps any D-Bus related error (connection, call, (de)serialization).
    pub(crate) fn dbus(err: impl std::fmt::Display) -> Self {
        Self::DBus(err.to_string())
    }
}

//! Cooperative cancellation: a request is cancelled as soon as a newer one is issued.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::PreviewError;

/// Handle on "is request `id` still the latest?".
#[derive(Debug, Clone)]
pub(crate) struct Cancel {
    latest: Arc<AtomicU64>,
    id: u64,
}

impl Cancel {
    pub(crate) fn new(latest: Arc<AtomicU64>, id: u64) -> Self {
        Self { latest, id }
    }

    /// A token that is never cancelled (tests, one-shot calls).
    #[cfg(test)]
    pub(crate) fn never() -> Self {
        Self::new(Arc::new(AtomicU64::new(1)), 1)
    }

    /// True once a newer request exists (or the service is shutting down).
    pub(crate) fn is_cancelled(&self) -> bool {
        // A plain flag that guards no other memory, so `Relaxed` is enough.
        self.latest.load(Ordering::Relaxed) != self.id
    }

    /// `Err(Cancelled)` when superseded; for use with `?` between steps.
    pub(crate) fn check(&self) -> Result<(), PreviewError> {
        if self.is_cancelled() {
            Err(PreviewError::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_flips_when_latest_moves() {
        let latest = Arc::new(AtomicU64::new(3));
        let token = Cancel::new(Arc::clone(&latest), 3);
        assert!(!token.is_cancelled());
        assert!(token.check().is_ok());
        latest.store(4, Ordering::Relaxed);
        assert!(token.is_cancelled());
        assert!(matches!(token.check(), Err(PreviewError::Cancelled)));
    }
}

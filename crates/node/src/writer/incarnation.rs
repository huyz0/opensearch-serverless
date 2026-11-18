//! Whether the index a shard belongs to is still the one it was opened
//! for: a delete and a recreate keep the name and change the uuid, and a
//! writer must not go on serving the deleted one.
//!
//! The Java's `ensureIncarnationLive`: one descriptor read, and within
//! the fence window after it, none.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use oss_meta::DescriptorStore;

/// How long a confirmed incarnation is trusted: the Java's
/// `INCARNATION_FENCE_MILLIS`.
pub const FENCE_WINDOW: Duration = Duration::from_secs(1);

/// What a writer asks before it writes: is this uuid the live one?
pub trait Incarnation: Send + Sync {
    /// Whether index `name` is currently the incarnation `uuid`.
    ///
    /// # Errors
    ///
    /// The descriptor could not be read.
    fn live(&self, name: &str, uuid: &str) -> Result<bool, String>;
}

/// [`Incarnation`] over the descriptor register: one read.
pub struct DescriptorIncarnation {
    descriptors: Arc<DescriptorStore>,
}

impl DescriptorIncarnation {
    /// The check over `descriptors`.
    pub fn new(descriptors: Arc<DescriptorStore>) -> Self {
        Self { descriptors }
    }
}

impl Incarnation for DescriptorIncarnation {
    fn live(&self, name: &str, uuid: &str) -> Result<bool, String> {
        // ALLOC: one descriptor's bytes a read.
        let mut buf = vec![0u8; 64 << 10];
        let record = self
            .descriptors
            .get(name, &mut buf)
            .map_err(|e| e.to_string())?;
        Ok(record.is_some_and(|r| r.uuid == uuid))
    }
}

/// An [`Incarnation`] remembered for a fence window.
pub(super) struct Fenced {
    check: Arc<dyn Incarnation>,
    window: Duration,
    confirmed: Mutex<Vec<(String, Instant)>>,
}

impl Fenced {
    pub(super) fn new(check: Arc<dyn Incarnation>, window: Duration) -> Self {
        Self {
            check,
            window,
            // ALLOC: one entry an index confirmed.
            confirmed: Mutex::new(Vec::new()),
        }
    }

    /// Confirms the incarnation, reading only outside the window.
    pub(super) fn ensure(&self, name: &str, uuid: &str) -> Result<bool, String> {
        {
            let confirmed = self
                .confirmed
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if confirmed
                .iter()
                .any(|(u, at)| u == uuid && at.elapsed() < self.window)
            {
                return Ok(true);
            }
        }
        let live = self.check.live(name, uuid)?;
        let mut confirmed = self
            .confirmed
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let window = self.window;
        // Entries past the window are dropped here, so the list holds only
        // the indexes written within the last window.
        confirmed.retain(|(u, at)| u != uuid && at.elapsed() < window);
        if live {
            // ALLOC: one entry an index confirmed.
            confirmed.push((uuid.to_owned(), Instant::now()));
        }
        Ok(live)
    }
}

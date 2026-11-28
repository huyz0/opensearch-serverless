//! A monitor's alerts: one register holding the set the monitor has open,
//! changed by compare-and-swap, so two nodes recording a run of the same
//! monitor cannot both win. The set is text the node reads and writes; this
//! module reads it, bounds it and swaps it. One read; a change is one read
//! and one swap.

use std::sync::Arc;

use oss_blob::register::ABSENT_GENERATION;
use oss_blob::{BlobStore, SwapOutcome};
use oss_format::{MAX_ALERT_SET_BYTES, alert_set_key, check_alert_set};

use crate::error::MetaError;

/// How many times a change reads the set and swaps before giving up.
const ATTEMPTS: u32 = 10;

/// One monitor's alert register.
pub struct Alerts {
    store: Arc<dyn BlobStore>,
    key: String,
}

impl Alerts {
    /// The alerts of `monitor` of `tenant` under `base`.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a base, tenant or monitor id that makes no key.
    pub fn new(
        store: &Arc<dyn BlobStore>,
        base: &str,
        tenant: &str,
        monitor: &str,
    ) -> Result<Self, MetaError> {
        Ok(Self {
            store: Arc::clone(store),
            key: alert_set_key(base, tenant, monitor)?,
        })
    }

    fn read_with_generation(&self) -> Result<(Vec<u8>, u64), MetaError> {
        // ALLOC: one set's worth, the most a register here holds, per read.
        let mut buf = vec![0u8; MAX_ALERT_SET_BYTES];
        match self.store.read_register(&self.key, &mut buf)? {
            None => Ok((Vec::new(), ABSENT_GENERATION)),
            Some(read) => {
                buf.truncate(read.len);
                check_alert_set(&buf)?;
                Ok((buf, read.generation))
            }
        }
    }

    /// The set as stored, empty when the monitor has had no alert: one request.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for stored bytes that are not a valid set;
    /// [`MetaError::Blob`].
    pub fn read(&self) -> Result<Vec<u8>, MetaError> {
        Ok(self.read_with_generation()?.0)
    }

    /// Reads the set, hands it to `edit` and swaps in what `edit` returns;
    /// `None` leaves the set as it is and sends no write. Reads again when
    /// another writer swapped first, so `edit` may run more than once.
    /// Returns whether a set was written.
    ///
    /// # Errors
    ///
    /// [`MetaError::Contended`] after repeated lost swaps;
    /// [`MetaError::Format`] for a set past its bound or not UTF-8, or one
    /// `edit` refuses; [`MetaError::Blob`].
    pub fn change(
        &self,
        mut edit: impl FnMut(&[u8]) -> Result<Option<Vec<u8>>, MetaError>,
    ) -> Result<bool, MetaError> {
        for _ in 0..ATTEMPTS {
            let (current, generation) = self.read_with_generation()?;
            let Some(next) = edit(&current)? else {
                return Ok(false);
            };
            check_alert_set(&next)?;
            let outcome = if generation == ABSENT_GENERATION {
                self.store.create_register(&self.key, &next)?
            } else {
                self.store.swap_register(&self.key, generation, &next)?
            };
            if matches!(outcome, SwapOutcome::Applied(_)) {
                return Ok(true);
            }
        }
        Err(MetaError::Contended)
    }
}

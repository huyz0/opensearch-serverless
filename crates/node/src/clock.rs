//! The wall clock, in milliseconds since the epoch.

use std::time::{SystemTime, UNIX_EPOCH};

/// Now, in milliseconds since the epoch; 0 for a clock before it.
pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

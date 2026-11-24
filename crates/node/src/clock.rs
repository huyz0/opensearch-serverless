//! The wall clock, in milliseconds since the epoch.

use std::time::{SystemTime, UNIX_EPOCH};

/// Now, in milliseconds since the epoch; 0 for a clock before it.
pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Milliseconds since this process first asked, from a clock that never goes
/// back: for ages and intervals, not for dates.
pub(crate) fn monotonic_millis() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(std::time::Instant::now);
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

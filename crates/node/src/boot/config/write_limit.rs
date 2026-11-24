//! The `OSS_WRITE_*` variables: what limits the writes waiting on the log.

use oss_writer::admission::LimiterConfig;

use super::NodeConfig;

/// Reads a bounded number: its key, default, lowest and highest.
pub(in crate::boot) type Bounded<'a> = &'a dyn Fn(&str, &str, u64, u64) -> Result<u64, String>;

/// Reads the limiter's four values through `bounded` (key, default, low,
/// high), which refuses a value out of its range.
pub(in crate::boot) fn read(bounded: Bounded<'_>) -> Result<LimiterConfig, String> {
    let count = |key: &str, default: &str, high: u64| {
        bounded(key, default, 1, high).map(|v| usize::try_from(v).unwrap_or(usize::MAX))
    };
    Ok(LimiterConfig {
        target_millis: bounded("OSS_WRITE_TARGET_MS", "1000", 1, 3_600_000)?,
        budget_millis: bounded("OSS_WRITE_BUDGET_MS", "5000", 1, 3_600_000)?,
        min_in_flight: count("OSS_WRITE_MIN_IN_FLIGHT", "4", 1 << 20)?,
        max_in_flight: count("OSS_WRITE_MAX_IN_FLIGHT", "1024", 1 << 30)?,
    })
}

/// Gives the assembled node what this node's variables set for its writers.
pub(in crate::boot) fn apply(from: &NodeConfig, to: &mut crate::assemble::Config) {
    to.hibernate_after_millis = from.hibernate_after_millis;
    to.write_limit = from.write_limit;
}

//! The `OSS_SEARCH_*` variables: the gate that orders searches under
//! pressure (ADR 0047).

use super::write_limit::Bounded;
use crate::search_slots::Config;

/// The REST server's workers: what the gate's slots and waiters must leave
/// headroom in.
fn workers() -> usize {
    oss_rest::ServerConfig::default().workers
}

/// Reads the gate's four values through `bounded`; with no slots nothing
/// else is checked.
pub(in crate::boot) fn read(bounded: Bounded<'_>) -> Result<Config, String> {
    let count = |v: u64| usize::try_from(v).unwrap_or(usize::MAX);
    let slots = count(bounded("OSS_SEARCH_SLOTS", "0", 0, 10_000)?);
    let fair_share_pct = count(bounded("OSS_SEARCH_FAIR_SHARE_PCT", "25", 1, 100)?);
    let wait_millis = bounded("OSS_SEARCH_WAIT_MS", "1000", 0, 60_000)?;
    let pct = count(bounded("OSS_SEARCH_MAX_WAITERS_PCT", "50", 0, 100)?);
    let workers = workers();
    let mut waiter_cap = workers.saturating_mul(pct).div_ceil(100);
    if pct > 0 {
        waiter_cap = waiter_cap.max(1);
    }
    if slots > 0 && slots.saturating_add(waiter_cap) >= workers {
        return Err(format!(
            "OSS_SEARCH_SLOTS [{slots}] and the waiter cap [{waiter_cap}] must stay below the {workers} workers"
        ));
    }
    Ok(Config {
        slots,
        fair_share_pct,
        wait_millis,
        waiter_cap,
    })
}

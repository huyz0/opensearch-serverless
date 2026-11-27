//! The opens of pinned indices a node refused, counted by why
//! (docs/specs/extension-refusal-metric): a mis-deployed fleet shows as
//! refusals here before it shows as an unserved shard anywhere else. The
//! totals are per process, which is per node, and each count is one atomic
//! add on an error path; nothing here reads or writes a store.

use std::sync::atomic::{AtomicU64, Ordering};

static BUSY: AtomicU64 = AtomicU64::new(0);
static UNAVAILABLE: AtomicU64 = AtomicU64::new(0);

/// Counts a refused open whose reason is `why`: for want of pool room to
/// compile a module (`busy`, a retry may succeed) or any other cause
/// (`unavailable`).
pub fn record(why: &str) {
    let total = if why.contains(crate::search_modules::NO_CAPACITY) {
        &BUSY
    } else {
        &UNAVAILABLE
    };
    total.fetch_add(1, Ordering::Relaxed);
}

/// The totals so far: (busy, unavailable).
#[must_use]
pub fn totals() -> (u64, u64) {
    (
        BUSY.load(Ordering::Relaxed),
        UNAVAILABLE.load(Ordering::Relaxed),
    )
}

//! What this process has counted of `knn` queries, for the plugin's stats API.
//!
//! One relaxed atomic add a `knn` node run; nothing is allocated and no
//! request is made. The counters are process-wide because the stats API
//! answers for the node.

use std::sync::atomic::{AtomicU64, Ordering};

/// Counts of the `knn` queries run.
#[derive(Debug, Default)]
pub struct Counters {
    queries: AtomicU64,
    filtered: AtomicU64,
}

/// A copy of the counters at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    /// `knn` nodes run, with or without a filter.
    pub queries: u64,
    /// Of those, the ones that carried a `filter`.
    pub filtered: u64,
}

impl Counters {
    /// Counts one `knn` node run.
    pub fn record(&self, with_filter: bool) {
        self.queries.fetch_add(1, Ordering::Relaxed);
        if with_filter {
            self.filtered.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The counts now.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            queries: self.queries.load(Ordering::Relaxed),
            filtered: self.filtered.load(Ordering::Relaxed),
        }
    }
}

static PROCESS: Counters = Counters {
    queries: AtomicU64::new(0),
    filtered: AtomicU64::new(0),
};

/// Counts one `knn` node run in this process.
pub(crate) fn record(with_filter: bool) {
    PROCESS.record(with_filter);
}

/// This process's counts.
pub fn snapshot() -> Snapshot {
    PROCESS.snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filtered_query_counts_as_a_query_and_as_filtered() {
        let c = Counters::default();
        assert_eq!(
            c.snapshot(),
            Snapshot {
                queries: 0,
                filtered: 0
            }
        );
        c.record(false);
        c.record(true);
        c.record(true);
        assert_eq!(
            c.snapshot(),
            Snapshot {
                queries: 3,
                filtered: 2
            }
        );
    }
}

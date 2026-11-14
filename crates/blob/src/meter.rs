//! Request counters for every S3 call a node makes.
//!
//! Ported from the serverless shell's `MeteredBlobStore`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/store/MeteredBlobStore.java`),
//! split by S3 request kind so price classes can be totalled. Its byte and
//! error counters are not ported yet.
//!
//! A store implementation calls [`Meter::record`] once per attempt, retries
//! included, before it returns. Tests assert on the counts ("this write issued
//! exactly one PUT"), so a change that adds a request fails a test instead of
//! showing up on a bill.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::op::{PriceClass, S3Op};

/// Lock-free per-request-kind counters.
#[derive(Debug, Default)]
pub struct Meter {
    counts: [AtomicU64; S3Op::ALL.len()],
}

impl Meter {
    /// A meter with every count at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Counts one attempt of `op`.
    pub fn record(&self, op: S3Op) {
        self.counts[op.index()].fetch_add(1, Ordering::Relaxed);
    }

    /// Attempts of `op` recorded so far.
    pub fn count(&self, op: S3Op) -> u64 {
        self.counts[op.index()].load(Ordering::Relaxed)
    }

    /// Attempts recorded so far across every kind in `class`. Saturates
    /// rather than wrapping.
    pub fn class_total(&self, class: PriceClass) -> u64 {
        S3Op::ALL
            .iter()
            .filter(|op| op.price_class() == class)
            .fold(0u64, |sum, op| sum.saturating_add(self.count(*op)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_each_kind_separately() {
        let meter = Meter::new();
        meter.record(S3Op::Put);
        meter.record(S3Op::GetRange);
        meter.record(S3Op::GetRange);
        assert_eq!(meter.count(S3Op::Put), 1);
        assert_eq!(meter.count(S3Op::GetRange), 2);
        assert_eq!(meter.count(S3Op::List), 0);
    }

    #[test]
    fn list_counts_toward_the_put_class() {
        let meter = Meter::new();
        meter.record(S3Op::Put);
        meter.record(S3Op::List);
        meter.record(S3Op::Get);
        meter.record(S3Op::Delete);
        assert_eq!(meter.class_total(PriceClass::PutClass), 2);
        assert_eq!(meter.class_total(PriceClass::GetClass), 1);
        assert_eq!(meter.class_total(PriceClass::Free), 1);
    }

    proptest::proptest! {
        /// Class totals partition the recorded attempts: they sum to the
        /// number recorded, and each equals its kinds' counts.
        #[test]
        fn class_totals_partition_every_attempt(ops in proptest::collection::vec(0usize..S3Op::ALL.len(), 0..200)) {
            let meter = Meter::new();
            for i in &ops {
                meter.record(S3Op::ALL[*i]);
            }
            let classes = [PriceClass::PutClass, PriceClass::GetClass, PriceClass::Free];
            let total: u64 = classes.iter().map(|c| meter.class_total(*c)).sum();
            proptest::prop_assert_eq!(total, ops.len() as u64);
            for op in S3Op::ALL {
                let expected = ops.iter().filter(|i| S3Op::ALL[**i] == op).count() as u64;
                proptest::prop_assert_eq!(meter.count(op), expected);
            }
        }
    }

    #[test]
    fn class_total_saturates() {
        let meter = Meter::new();
        meter.counts[S3Op::Put.index()].store(u64::MAX, Ordering::Relaxed);
        meter.record(S3Op::List);
        assert_eq!(meter.class_total(PriceClass::PutClass), u64::MAX);
    }
}

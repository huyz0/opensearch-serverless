//! Request counters for every S3 call a node makes.
//!
//! Ported from the serverless shell's `MeteredBlobStore`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/store/MeteredBlobStore.java`),
//! split by S3 request kind so price classes can be totalled, with its byte
//! and error counters. The Java's per-thread request attribution is not
//! ported yet.
//!
//! A store implementation calls [`Meter::record`] once per attempt, retries
//! included, before it returns. Tests assert on the counts ("this write issued
//! exactly one PUT"), so a change that adds a request fails a test instead of
//! showing up on a bill.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use crate::op::{PriceClass, S3Op};

/// Told of everything a [`Meter`] counts, as it is counted, on the thread
/// that made the request: how a per-index ledger learns who a request was
/// for (`crate::usage`).
pub trait MeterTap: Send + Sync + std::fmt::Debug {
    /// One attempt of `op` was made.
    fn request(&self, op: S3Op);
    /// `n` body bytes reached a GET's sink.
    fn bytes_read(&self, n: u64);
    /// `n` body bytes were committed by a PUT.
    fn bytes_written(&self, n: u64);
}

/// Lock-free per-request-kind counters, plus bytes moved and failures.
#[derive(Debug, Default)]
pub struct Meter {
    tap: OnceLock<Arc<dyn MeterTap>>,
    counts: [AtomicU64; S3Op::ALL.len()],
    bytes_read: AtomicU64,
    bytes_written: AtomicU64,
    errors: AtomicU64,
}

impl Meter {
    /// A meter with every count at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Has `tap` told of every count from now on; false when one is set
    /// already.
    pub fn set_tap(&self, tap: Arc<dyn MeterTap>) -> bool {
        self.tap.set(tap).is_ok()
    }

    /// Counts one attempt of `op`.
    pub fn record(&self, op: S3Op) {
        self.counts[op.index()].fetch_add(1, Ordering::Relaxed);
        if let Some(tap) = self.tap.get() {
            tap.request(op);
        }
    }

    /// Counts one attempt that failed: the store refused it, the caller's
    /// stream failed, or a conditional create found the key taken. Every
    /// failed attempt was also counted by [`Meter::record`].
    pub fn record_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts `n` body bytes delivered to a GET's sink. For a ranged GET
    /// that is the range, which says whether reads are lazy.
    pub fn add_bytes_read(&self, n: u64) {
        self.bytes_read.fetch_add(n, Ordering::Relaxed);
        if let Some(tap) = self.tap.get() {
            tap.bytes_read(n);
        }
    }

    /// Counts `n` body bytes a PUT committed. Unlike the Java, which adds
    /// the declared size before every attempt, failed and refused attempts
    /// add nothing, so this is bytes stored rather than bytes sent.
    pub fn add_bytes_written(&self, n: u64) {
        self.bytes_written.fetch_add(n, Ordering::Relaxed);
        if let Some(tap) = self.tap.get() {
            tap.bytes_written(n);
        }
    }

    /// Failed attempts recorded so far.
    pub fn errors(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }

    /// Body bytes delivered to GET sinks so far.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read.load(Ordering::Relaxed)
    }

    /// Body bytes accepted by PUTs so far.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written.load(Ordering::Relaxed)
    }

    /// Attempts recorded so far across every kind.
    pub fn total(&self) -> u64 {
        S3Op::ALL
            .iter()
            .fold(0u64, |sum, op| sum.saturating_add(self.count(*op)))
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
    fn counts_bytes_and_errors_apart_from_requests() {
        let meter = Meter::new();
        meter.record(S3Op::Get);
        meter.add_bytes_read(10);
        meter.add_bytes_read(5);
        meter.add_bytes_written(7);
        meter.record_error();
        assert_eq!(meter.bytes_read(), 15);
        assert_eq!(meter.bytes_written(), 7);
        assert_eq!(meter.errors(), 1);
        assert_eq!(meter.total(), 1);
    }

    #[test]
    fn class_total_saturates() {
        let meter = Meter::new();
        meter.counts[S3Op::Put.index()].store(u64::MAX, Ordering::Relaxed);
        meter.record(S3Op::List);
        assert_eq!(meter.class_total(PriceClass::PutClass), u64::MAX);
    }
}

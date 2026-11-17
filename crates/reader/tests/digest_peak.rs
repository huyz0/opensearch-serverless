//! Building, settling and reading a percentiles digest never holds more
//! than `digest_bytes` reserves for it (docs/specs/percentiles-agg/SPEC.md,
//! AC-5), whatever count of values arrives unmerged. Its own binary,
//! because the counting allocator is process-wide.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use lucene_aggs::tdigest::MergingDigest;
use oss_reader::aggs::metric::tdigest::{Digest, digest_bytes};
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

#[test]
fn a_digest_never_holds_more_than_it_reserves() {
    for compression in [100.0, 1000.0, 10_000.0] {
        let probe = MergingDigest::new(compression);
        let buffer = probe.temp_weight.len();
        drop(probe);
        // Up to a full buffer unmerged, then the same after merges.
        for n in [buffer / 2, buffer - 2, buffer * 3 + 7] {
            // Each case on a fresh thread, so the engine's per-thread sort
            // keys start empty and are counted.
            let (peak, bound) = std::thread::spawn(move || {
                let window = PeakWindow::start(&ALLOC);
                let mut engine = MergingDigest::new(compression);
                for i in 0..n {
                    // A spread with repeats, out of order.
                    engine.add(((i * 7919) % 1013) as f64).expect("not NaN");
                }
                let mut digest = Digest::from_engine(engine);
                digest.settle();
                let q = digest.quantile(0.5);
                let c = digest.cdf(500.0);
                assert!(q.is_finite() && c.is_finite());
                drop(digest);
                (window.peak_bytes(), digest_bytes(compression))
            })
            .join()
            .expect("measures");
            assert!(
                peak <= bound,
                "compression {compression}, {n} values: peak {peak} over {bound}"
            );
        }
    }
}

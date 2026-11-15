//! The counting allocator and peak assertions, measured as this binary's
//! global allocator. One test, so nothing else shares the counters.

use std::hint::black_box;

use oss_testkit::{
    CountingAllocator, PeakExceeded, PeakWindow, assert_peak_below, counting_allocator,
};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

fn check_peak_at_the_bound_and_one_byte_over() {
    // Warm the vector's size class so the window reads only the vector.
    drop(black_box(vec![1u8; 10_000]));
    let w = PeakWindow::start(&ALLOC);
    let v = black_box(vec![1u8; 10_000]);
    assert_eq!(w.live_bytes(), 10_000);
    drop(v);
    assert_eq!(w.live_bytes(), 0);
    assert!(w.allocated_bytes() >= 10_000);
    // The process already peaked above this window's baseline while the test
    // harness started, so open a fresh window whose peak is known: allocate
    // more than anything before it.
    let big = 64 << 20;
    let w = PeakWindow::start(&ALLOC);
    let before = w.peak_bytes();
    let v = black_box(vec![1u8; big]);
    let peak = w.peak_bytes();
    drop(v);
    assert!(peak >= big, "peak {peak} must cover the {big}-byte vector");
    assert!(peak <= big.saturating_add(before));
    assert_eq!(w.check_below(peak), Ok(peak));
    assert_eq!(
        w.check_below(peak.saturating_sub(1)),
        Err(PeakExceeded {
            peak,
            bound: peak.saturating_sub(1)
        })
    );
}

fn assert_peak_below_passes_and_panics_one_byte_under_the_work() {
    let size = 128 << 20;
    let ((), peak) = assert_peak_below(&ALLOC, usize::MAX, || drop(black_box(vec![1u8; size])));
    assert!(peak >= size);
    let over = std::panic::catch_unwind(|| {
        assert_peak_below(&ALLOC, size.saturating_sub(1), || {
            drop(black_box(vec![1u8; size]))
        });
    });
    let msg = over.expect_err("one byte under the vector must panic");
    let text = msg
        .downcast_ref::<String>()
        .map(String::as_str)
        .unwrap_or_default();
    assert!(text.starts_with("peak memory "), "unexpected panic: {text}");
}

#[test]
fn counting_allocator_reads_peaks_and_assertions_hold_at_the_bound() {
    check_peak_at_the_bound_and_one_byte_over();
    assert_peak_below_passes_and_panics_one_byte_under_the_work();
}

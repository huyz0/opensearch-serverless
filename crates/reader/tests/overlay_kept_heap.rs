//! The kept estimate against what a built overlay retains, measured by the
//! counting allocator. Its own binary, because the counting allocator is
//! process-wide and its maximum cannot be reset: measured beside the build
//! estimate's test (`overlay_heap.rs`) in one process, whichever ran first
//! would set the peak the other reads.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use oss_doc::mapping::Mapping;
use oss_format::WalRecord;
use oss_reader::overlay::{Overlay, estimate_bytes, kept_bytes};
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};

#[path = "common/heap_shapes.rs"]
mod heap_shapes;
use heap_shapes::{mapping, records, wide};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

#[test]
fn the_kept_estimate_bounds_what_a_built_overlay_retains() {
    let m = mapping();
    drop(Overlay::build(&m, &records(5, 50), 0, usize::MAX).expect("warm-up"));
    let mut shapes: Vec<(String, Mapping, Vec<WalRecord<'static>>)> = [
        ("one-small", 1, 100),
        ("100-small", 100, 100),
        ("100-1k", 100, 1_000),
        ("1000-small", 1_000, 100),
        ("1000-1k", 1_000, 1_000),
        ("100-64k", 100, 64_000),
        ("10-512k", 10, 512_000),
    ]
    .into_iter()
    .map(|(name, count, body)| (name.to_owned(), mapping(), records(count, body)))
    .collect();
    for (name, count, fields, value) in [
        ("random-100x20x50", 100, 20, 50),
        ("random-10x200x20", 10, 200, 20),
        ("random-1000x4x200", 1_000, 4, 200),
    ] {
        let (mapping, recs) = wide(count, fields, value);
        shapes.push((name.to_owned(), mapping, recs));
    }
    let mut report = Vec::new();
    for (name, mapping, recs) in &shapes {
        let window = PeakWindow::start(&ALLOC);
        let overlay = Overlay::build(mapping, recs, 0, usize::MAX).expect("build");
        let retained = window.live_bytes();
        let stored = overlay.stored_bytes();
        drop(overlay);
        report.push((
            name.as_str(),
            retained,
            kept_bytes(recs, stored),
            estimate_bytes(recs),
        ));
    }
    for (name, retained, stored, estimate) in &report {
        println!(
            "{name}: retained {retained} B, kept estimate {stored} B, build estimate {estimate} B"
        );
    }
    for (name, retained, kept, estimate) in &report {
        assert!(
            kept >= retained,
            "{name}: kept estimate {kept} under the {retained} B retained: {report:?}"
        );
        assert!(
            kept <= estimate,
            "{name}: kept {kept} over the build estimate {estimate}"
        );
    }
}

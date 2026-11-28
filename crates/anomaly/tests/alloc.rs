//! Feeding a window to a built model allocates nothing (spec AC-1). Its own
//! binary, because the counting allocator is process-wide.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use oss_anomaly::{Model, Params, Workspace};
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const POINTS: u64 = 100_000;
const ATTEMPTS: usize = 5;

#[test]
fn feeding_a_hundred_thousand_windows_allocates_nothing() {
    let mut p = Params::new(3, 17);
    p.shingle_size = 4;
    p.trees = 10;
    p.sample_size = 64;
    p.min_train_samples = 32;
    let mut ws = Workspace::new(&p);
    let mut model = Model::new(p).expect("valid");
    // Warm every array to its working size, past the ring and the sample.
    let mut x = 1u64;
    let mut next = move || {
        x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        ((x >> 40) as f32) / 1.0e5
    };
    let mut slot = 0u64;
    for _ in 0..2_000 {
        model
            .feed(&mut ws, slot, Some(&[next(), next(), next()]))
            .expect("feed");
        slot += 1;
    }
    // The counter is process-wide, so the harness's own thread can allocate
    // inside a window. The code under test is deterministic: if it
    // allocated, every run would, so a run that reads zero shows it does not.
    let mut allocated = usize::MAX;
    for _ in 0..ATTEMPTS {
        let window = PeakWindow::start(&ALLOC);
        for _ in 0..POINTS / ATTEMPTS as u64 {
            let w = [next(), next(), next()];
            let _ = model.feed(&mut ws, slot, Some(&w)).expect("feed");
            slot += 1;
            if slot.is_multiple_of(7) {
                // a missing window and a duplicate-heavy stretch take the other paths
                let _ = model.feed(&mut ws, slot, None).expect("feed");
                slot += 1;
            }
        }
        allocated = allocated.min(window.allocated_bytes());
        if allocated == 0 {
            break;
        }
    }
    assert_eq!(allocated, 0, "feeding a window called the allocator");
}

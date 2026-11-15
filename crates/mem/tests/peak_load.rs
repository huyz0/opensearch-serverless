//! Sustained load stays under the memory budget: every request reserves
//! before it allocates and is refused when its pool is full. Evidence for
//! invariant I-17. Its own binary, so no other test shares the counters.

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const THREADS: usize = 8;
const REQUESTS: usize = 2_000;
/// The request-body pool. Eight threads asking for up to 512 KiB each can
/// want 4 MiB at once, twice this, so the bound is reached.
const BUDGET: usize = 2 << 20;
const MAX_BODY: u64 = 512 << 10;
/// Heap the harness itself uses beside the bodies: spawned threads' handles
/// and names. Measured at about 1 KiB; 64 KiB of headroom.
const HARNESS: usize = 64 << 10;

#[test]
fn peak_heap_under_sustained_load_stays_within_the_budget() {
    let config = BudgetConfig::new(BUDGET)
        .with_pool(Pool::RequestBodies, BUDGET)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let refused = Arc::new(AtomicUsize::new(0));
    let served = Arc::new(AtomicUsize::new(0));

    let window = PeakWindow::start(&ALLOC);
    let workers: Vec<_> = (0..THREADS)
        .map(|t| {
            let budget = budget.clone();
            let (refused, served) = (Arc::clone(&refused), Arc::clone(&served));
            thread::spawn(move || {
                let mut seed = (t as u64)
                    .wrapping_mul(0x2545_F491_4F6C_DD1D)
                    .wrapping_add(7);
                for _ in 0..REQUESTS {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    let size = usize::try_from(seed % MAX_BODY)
                        .expect("fits usize")
                        .saturating_add(1);
                    let Ok(reservation) = budget.try_reserve(Pool::RequestBodies, size) else {
                        // The request is refused with 429; the client retries later.
                        refused.fetch_add(1, Ordering::Relaxed);
                        thread::yield_now();
                        continue;
                    };
                    // ALLOC: `size` bytes, reserved above.
                    let mut body = vec![0u8; reservation.bytes()];
                    for i in (0..body.len()).step_by(4096) {
                        body[i] = 1;
                    }
                    black_box(&body);
                    drop(body);
                    drop(reservation);
                    served.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();
    for w in workers {
        w.join().expect("worker panicked");
    }

    let peak = window.peak_bytes();
    let (refused, served) = (
        refused.load(Ordering::Relaxed),
        served.load(Ordering::Relaxed),
    );
    eprintln!("peak {peak} bytes, budget {BUDGET}, served {served}, refused {refused}");
    assert_eq!(
        served.saturating_add(refused),
        THREADS.saturating_mul(REQUESTS)
    );
    assert!(
        refused > 0,
        "no request was refused, so the budget was never reached"
    );
    assert!(
        peak <= BUDGET.saturating_add(HARNESS),
        "peak {peak} over budget {BUDGET} + {HARNESS}"
    );
    assert_eq!(budget.in_use(Pool::RequestBodies), 0);
}

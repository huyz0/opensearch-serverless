//! Steady-state checkout, return and reservation never call the global
//! allocator. Its own binary, so no other test shares the counters.

use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const CYCLES: usize = 10_000;
const ATTEMPTS: usize = 5;

#[test]
fn ten_thousand_checkouts_and_reservations_allocate_nothing() {
    let bytes = SMALL_BUFFER.saturating_mul(4);
    let config = BudgetConfig::new(bytes.saturating_mul(2))
        .with_pool(Pool::Transport, bytes)
        .and_then(|c| c.with_pool(Pool::RequestBodies, bytes))
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let pool = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 4).expect("fits");

    // The counter is process-wide, so the test harness's own thread can
    // allocate inside a window. The code under test is deterministic: if it
    // allocated, every run would, so a run that reads zero shows it does not
    // and only a run that always reads above zero fails.
    let mut allocated = usize::MAX;
    for _ in 0..ATTEMPTS {
        let window = PeakWindow::start(&ALLOC);
        for i in 0..CYCLES {
            let mut a = pool.try_checkout().expect("free");
            let b = pool.try_checkout().expect("free");
            let r = budget
                .try_reserve(Pool::RequestBodies, i.saturating_add(1))
                .expect("fits");
            a[0] = b[0].wrapping_add(1);
            drop((a, b, r));
        }
        allocated = allocated.min(window.allocated_bytes());
        if allocated == 0 {
            break;
        }
    }
    assert_eq!(allocated, 0, "steady state called the allocator");
    assert_eq!(pool.available(), 4);
}

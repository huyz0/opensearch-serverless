//! Concurrent checkouts and reservations never overshoot their bounds.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool};

const THREADS: usize = 16;
const ROUNDS: usize = 10_000;
const BUFFERS: usize = 8;
const BUFFER: usize = 4096;
const BODY_LIMIT: usize = 64 << 10;

/// Counts what is held now and remembers the most ever held at once.
#[derive(Default)]
struct HighWater {
    now: AtomicUsize,
    max: AtomicUsize,
}

impl HighWater {
    fn add(&self, n: usize) {
        let now = self.now.fetch_add(n, Ordering::SeqCst).saturating_add(n);
        self.max.fetch_max(now, Ordering::SeqCst);
    }

    fn sub(&self, n: usize) {
        self.now.fetch_sub(n, Ordering::SeqCst);
    }
}

#[test]
fn sixteen_threads_never_exceed_the_buffer_count_or_pool_limits() {
    let budget = MemoryBudget::new(&config());
    let buffers = BufferPool::new(&budget, Pool::Transport, BUFFER, BUFFERS).expect("fits");
    let held_buffers = Arc::new(HighWater::default());
    let held_bytes = Arc::new(HighWater::default());
    let refused = Arc::new(AtomicUsize::new(0));

    let workers: Vec<_> = (0..THREADS)
        .map(|t| {
            let (budget, buffers) = (budget.clone(), buffers.clone());
            let (held_buffers, held_bytes, refused) = (
                Arc::clone(&held_buffers),
                Arc::clone(&held_bytes),
                Arc::clone(&refused),
            );
            thread::spawn(move || {
                let mut seed = (t as u64)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .wrapping_add(1);
                for _ in 0..ROUNDS {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    let size = usize::try_from(seed % 16_384)
                        .expect("fits usize")
                        .saturating_add(1);
                    match (
                        buffers.try_checkout(),
                        budget.try_reserve(Pool::RequestBodies, size),
                    ) {
                        (Ok(mut buf), Ok(body)) => {
                            held_buffers.add(1);
                            held_bytes.add(body.bytes());
                            assert!(budget.in_use(Pool::RequestBodies) <= BODY_LIMIT);
                            buf[0] = buf[0].wrapping_add(1);
                            thread::yield_now();
                            held_bytes.sub(body.bytes());
                            held_buffers.sub(1);
                        }
                        (Ok(buf), Err(_)) => {
                            held_buffers.add(1);
                            held_buffers.sub(1);
                            drop(buf);
                            refused.fetch_add(1, Ordering::Relaxed);
                        }
                        (Err(_), _) => {
                            refused.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            })
        })
        .collect();
    for w in workers {
        w.join().expect("worker panicked");
    }

    assert!(held_buffers.max.load(Ordering::SeqCst) <= BUFFERS);
    assert!(held_bytes.max.load(Ordering::SeqCst) <= BODY_LIMIT);
    assert!(
        refused.load(Ordering::Relaxed) > 0,
        "the bounds were never reached"
    );
    assert_eq!(buffers.available(), BUFFERS, "every buffer came back");
    assert_eq!(
        budget.in_use(Pool::RequestBodies),
        0,
        "every byte came back"
    );
    assert_eq!(
        budget.in_use(Pool::Transport),
        BUFFERS.saturating_mul(BUFFER)
    );
}

fn config() -> BudgetConfig {
    let buffers = BUFFERS.saturating_mul(BUFFER);
    BudgetConfig::new(BODY_LIMIT.saturating_add(buffers))
        .with_pool(Pool::RequestBodies, BODY_LIMIT)
        .and_then(|c| c.with_pool(Pool::Transport, buffers))
        .expect("pools fit the total")
}

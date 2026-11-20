//! A pool of fixed-size buffers, allocated once and charged to a memory pool.

use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::{Exhausted, MemoryBudget, Pool, Reservation};

/// The small I/O buffer size: 64 KiB.
pub const SMALL_BUFFER: usize = 64 << 10;
/// The large I/O buffer size: 1 MiB.
pub const LARGE_BUFFER: usize = 1 << 20;

#[derive(Debug)]
struct Inner {
    free: Mutex<Vec<Box<[u8]>>>,
    /// Signalled when a buffer comes back.
    returned: Condvar,
    size: usize,
    count: usize,
    charge: Reservation,
}

impl Inner {
    /// Locks the free list. A panic while it was held cannot leave it
    /// half-changed (a push or a pop either happened or did not), so a
    /// poisoned lock is recovered rather than refused.
    fn free(&self) -> MutexGuard<'_, Vec<Box<[u8]>>> {
        self.free.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// `count` buffers of `size` bytes each, allocated when the pool is built.
///
/// The pool reserves `size * count` bytes from its memory pool for as long as
/// any handle or checked-out buffer is alive. Checkout and return move a
/// buffer between the free list and a guard and never call the global
/// allocator. Cheap to clone; clones share the buffers.
#[derive(Clone, Debug)]
pub struct BufferPool {
    inner: Arc<Inner>,
}

impl BufferPool {
    /// Reserves `size * count` bytes from `pool`, then allocates the buffers.
    ///
    /// Fails with [`Exhausted`] when the pool cannot hold them (or the product
    /// overflows); nothing is allocated then.
    pub fn new(
        budget: &MemoryBudget,
        pool: Pool,
        size: usize,
        count: usize,
    ) -> Result<Self, Exhausted> {
        let bytes = size.checked_mul(count).ok_or(Exhausted {
            pool,
            requested: usize::MAX,
            in_use: budget.in_use(pool),
            limit: budget.limit(pool),
        })?;
        let charge = budget.try_reserve(pool, bytes)?;
        // ALLOC: `count` buffers of `size` bytes, reserved above.
        let mut free = Vec::with_capacity(count);
        free.extend((0..count).map(|_| vec![0u8; size].into_boxed_slice()));
        Ok(Self {
            inner: Arc::new(Inner {
                free: Mutex::new(free),
                returned: Condvar::new(),
                size,
                count,
                charge,
            }),
        })
    }

    /// Takes a free buffer, or fails with [`Exhausted`] when every buffer is
    /// checked out. The buffer returns to the pool when the guard drops.
    ///
    /// A returned buffer keeps whatever its last user wrote.
    pub fn try_checkout(&self) -> Result<PooledBuffer, Exhausted> {
        let buf = self.inner.free().pop();
        match buf {
            Some(buf) => Ok(PooledBuffer {
                buf: Some(buf),
                pool: Arc::clone(&self.inner),
            }),
            None => Err(Exhausted {
                pool: self.inner.charge.pool(),
                requested: self.inner.size,
                in_use: self.inner.charge.bytes(),
                limit: self.inner.charge.bytes(),
            }),
        }
    }

    /// Like [`Self::try_checkout`], but waits up to `wait` for a buffer to
    /// come back when every one is checked out. A zero `wait` is
    /// `try_checkout`.
    ///
    /// The wait cannot deadlock a caller that holds no buffer while it waits.
    pub fn checkout_within(&self, wait: Duration) -> Result<PooledBuffer, Exhausted> {
        let Some(deadline) = Instant::now().checked_add(wait) else {
            return self.try_checkout();
        };
        let mut free = self.inner.free();
        loop {
            if let Some(buf) = free.pop() {
                return Ok(PooledBuffer {
                    buf: Some(buf),
                    pool: Arc::clone(&self.inner),
                });
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                drop(free);
                return self.try_checkout();
            }
            free = self
                .inner
                .returned
                .wait_timeout(free, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Each buffer's size in bytes.
    pub fn buffer_size(&self) -> usize {
        self.inner.size
    }

    /// How many buffers the pool holds in all.
    pub fn capacity(&self) -> usize {
        self.inner.count
    }

    /// How many buffers are free now.
    pub fn available(&self) -> usize {
        self.inner.free().len()
    }
}

/// A buffer checked out of a [`BufferPool`], returned to it on drop.
#[derive(Debug)]
pub struct PooledBuffer {
    buf: Option<Box<[u8]>>,
    pool: Arc<Inner>,
}

impl Deref for PooledBuffer {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.buf.as_deref().unwrap_or_default()
    }
}

impl DerefMut for PooledBuffer {
    fn deref_mut(&mut self) -> &mut [u8] {
        self.buf.as_deref_mut().unwrap_or_default()
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        if let Some(buf) = self.buf.take() {
            // The free list was allocated with room for every buffer, so this
            // push never reallocates.
            self.pool.free().push(buf);
            self.pool.returned.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BudgetConfig;

    fn budget(bytes: usize) -> MemoryBudget {
        MemoryBudget::new(
            &BudgetConfig::new(bytes)
                .with_pool(Pool::Transport, bytes)
                .expect("fits"),
        )
    }

    #[test]
    fn construction_charges_size_times_count() {
        let b = budget(4 * 1024);
        let p = BufferPool::new(&b, Pool::Transport, 1024, 4).expect("exactly fits");
        assert_eq!(b.in_use(Pool::Transport), 4 * 1024);
        assert_eq!((p.buffer_size(), p.capacity(), p.available()), (1024, 4, 4));
    }

    #[test]
    fn construction_one_buffer_too_many_is_refused() {
        let b = budget(4 * 1024);
        let err = BufferPool::new(&b, Pool::Transport, 1024, 5).expect_err("one buffer over");
        assert_eq!(
            err,
            Exhausted {
                pool: Pool::Transport,
                requested: 5 * 1024,
                in_use: 0,
                limit: 4 * 1024
            }
        );
        assert_eq!(b.in_use(Pool::Transport), 0);
    }

    #[test]
    fn construction_with_overflowing_size_is_refused() {
        let b = budget(1024);
        let err = BufferPool::new(&b, Pool::Transport, usize::MAX, 2).expect_err("overflows");
        assert_eq!((err.requested, err.limit), (usize::MAX, 1024));
        assert!(err.exceeds_limit());
    }

    #[test]
    fn a_checkout_within_a_wait_takes_a_buffer_that_comes_back() {
        let b = budget(64);
        let p = BufferPool::new(&b, Pool::Transport, 64, 1).expect("fits");
        let held = p.try_checkout().expect("free");
        let waiter = {
            let p = p.clone();
            std::thread::spawn(move || p.checkout_within(Duration::from_secs(30)).is_ok())
        };
        std::thread::sleep(Duration::from_millis(50));
        drop(held);
        assert!(waiter.join().expect("joins"), "the buffer came back");
    }

    #[test]
    fn a_checkout_within_a_wait_gives_up_when_none_comes_back() {
        let b = budget(64);
        let p = BufferPool::new(&b, Pool::Transport, 64, 1).expect("fits");
        let _held = p.try_checkout().expect("free");
        assert!(p.checkout_within(Duration::from_millis(20)).is_err());
        assert!(p.checkout_within(Duration::ZERO).is_err());
    }

    #[test]
    fn count_checkouts_succeed_and_one_more_is_refused() {
        let b = budget(3 * 64);
        let p = BufferPool::new(&b, Pool::Transport, 64, 3).expect("fits");
        let held: Vec<PooledBuffer> = (0..3).map(|_| p.try_checkout().expect("free")).collect();
        assert_eq!(p.available(), 0);
        let err = p.try_checkout().expect_err("count + 1");
        assert_eq!(
            err,
            Exhausted {
                pool: Pool::Transport,
                requested: 64,
                in_use: 192,
                limit: 192
            }
        );
        drop(held);
        assert_eq!(p.available(), 3);
        assert!(p.try_checkout().is_ok());
    }

    #[test]
    fn returning_one_guard_frees_one_checkout() {
        let b = budget(2 * 64);
        let p = BufferPool::new(&b, Pool::Transport, 64, 2).expect("fits");
        let first = p.try_checkout().expect("free");
        let _second = p.try_checkout().expect("free");
        assert!(p.try_checkout().is_err());
        drop(first);
        assert!(p.try_checkout().is_ok());
    }

    #[test]
    fn buffers_are_writable_full_size_and_keep_contents() {
        let b = budget(64);
        let p = BufferPool::new(&b, Pool::Transport, 64, 1).expect("fits");
        let mut buf = p.try_checkout().expect("free");
        assert_eq!(buf.len(), 64);
        buf[63] = 9;
        drop(buf);
        let again = p.try_checkout().expect("free");
        assert_eq!(again[63], 9);
    }

    #[test]
    fn charge_is_released_when_pool_and_guards_drop() {
        let b = budget(128);
        let p = BufferPool::new(&b, Pool::Transport, 64, 2).expect("fits");
        let guard = p.try_checkout().expect("free");
        drop(p);
        assert_eq!(
            b.in_use(Pool::Transport),
            128,
            "a live guard keeps the charge"
        );
        drop(guard);
        assert_eq!(b.in_use(Pool::Transport), 0);
    }

    #[test]
    fn poisoned_free_list_is_recovered() {
        let b = budget(64);
        let p = BufferPool::new(&b, Pool::Transport, 64, 1).expect("fits");
        let q = p.clone();
        let _ = std::thread::spawn(move || {
            let _held = q.inner.free();
            panic!("poison the lock");
        })
        .join();
        assert!(p.inner.free.is_poisoned());
        let buf = p.try_checkout().expect("recovered");
        drop(buf);
        assert_eq!(p.available(), 1);
    }

    #[test]
    fn standard_sizes() {
        assert_eq!(SMALL_BUFFER, 65_536);
        assert_eq!(LARGE_BUFFER, 1_048_576);
    }
}

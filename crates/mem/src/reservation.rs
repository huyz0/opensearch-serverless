//! A reservation of bytes from one pool, released on drop.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{Exhausted, Pool};

/// One pool's limit and the bytes currently reserved from it.
#[derive(Debug)]
pub(crate) struct PoolCounter {
    pool: Pool,
    limit: usize,
    used: AtomicUsize,
}

impl PoolCounter {
    pub(crate) const fn new(pool: Pool, limit: usize) -> Self {
        Self {
            pool,
            limit,
            used: AtomicUsize::new(0),
        }
    }

    pub(crate) fn limit(&self) -> usize {
        self.limit
    }

    pub(crate) fn in_use(&self) -> usize {
        self.used.load(Ordering::Acquire)
    }

    /// Adds `bytes` to the pool if the total stays at or under the limit.
    pub(crate) fn try_take(&self, bytes: usize) -> Result<(), Exhausted> {
        let mut current = self.used.load(Ordering::Acquire);
        loop {
            let next = match current.checked_add(bytes) {
                Some(next) if next <= self.limit => next,
                _ => {
                    return Err(Exhausted {
                        pool: self.pool,
                        requested: bytes,
                        in_use: current,
                        limit: self.limit,
                    });
                }
            };
            match self.used.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => current = actual,
            }
        }
    }

    /// Returns `bytes` previously taken.
    pub(crate) fn give_back(&self, bytes: usize) {
        let before = self.used.fetch_sub(bytes, Ordering::AcqRel);
        debug_assert!(before >= bytes, "released more than reserved");
    }
}

/// Bytes reserved from one pool of a [`crate::MemoryBudget`].
///
/// Taken before the memory is allocated and released when dropped, so the
/// pool's count can never fall behind what is actually held.
#[derive(Debug)]
#[must_use = "dropping a reservation releases it at once"]
pub struct Reservation {
    counter: Arc<PoolCounter>,
    bytes: usize,
}

impl Reservation {
    /// Takes `bytes` from `counter`, or fails without changing it.
    pub(crate) fn take(counter: &Arc<PoolCounter>, bytes: usize) -> Result<Self, Exhausted> {
        counter.try_take(bytes)?;
        Ok(Self {
            counter: Arc::clone(counter),
            bytes,
        })
    }

    /// The pool this reservation is charged to.
    pub fn pool(&self) -> Pool {
        self.counter.pool
    }

    /// Bytes this reservation holds.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Grows the reservation by `extra` bytes, or fails and leaves it unchanged.
    ///
    /// For a body or buffer that grows as input streams in: grow first, then
    /// allocate.
    pub fn try_grow(&mut self, extra: usize) -> Result<(), Exhausted> {
        let grown = self.bytes.checked_add(extra).ok_or(Exhausted {
            pool: self.counter.pool,
            requested: extra,
            in_use: self.counter.in_use(),
            limit: self.counter.limit,
        })?;
        self.counter.try_take(extra)?;
        self.bytes = grown;
        Ok(())
    }

    /// Releases `bytes` of the reservation now, keeping the rest. Releases
    /// everything it holds when `bytes` is larger.
    pub fn shrink(&mut self, bytes: usize) {
        let released = bytes.min(self.bytes);
        self.counter.give_back(released);
        self.bytes = self.bytes.saturating_sub(released);
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.counter.give_back(self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counter(limit: usize) -> Arc<PoolCounter> {
        Arc::new(PoolCounter::new(Pool::WriteBuffers, limit))
    }

    #[test]
    fn take_up_to_the_limit_then_refuse_one_more_byte() {
        let c = counter(100);
        let r = Reservation::take(&c, 100).expect("exactly the limit fits");
        assert_eq!(r.bytes(), 100);
        assert_eq!(r.pool(), Pool::WriteBuffers);
        let err = Reservation::take(&c, 1).expect_err("one past the limit");
        assert_eq!(
            err,
            Exhausted {
                pool: Pool::WriteBuffers,
                requested: 1,
                in_use: 100,
                limit: 100
            }
        );
    }

    #[test]
    fn take_refuses_a_size_that_overflows() {
        let c = counter(usize::MAX);
        let _r = Reservation::take(&c, 1).expect("fits");
        let err = Reservation::take(&c, usize::MAX).expect_err("overflows");
        assert_eq!(err.in_use, 1);
        assert_eq!(c.in_use(), 1);
    }

    #[test]
    fn zero_bytes_always_fit() {
        let c = counter(0);
        let r = Reservation::take(&c, 0).expect("zero fits a zero pool");
        assert_eq!(r.bytes(), 0);
        assert!(Reservation::take(&c, 1).is_err());
    }

    #[test]
    fn grow_to_the_limit_then_refuse_one_more_byte_unchanged() {
        let c = counter(100);
        let mut r = Reservation::take(&c, 40).expect("fits");
        r.try_grow(60).expect("grows to exactly the limit");
        assert_eq!((r.bytes(), c.in_use()), (100, 100));
        let err = r.try_grow(1).expect_err("one past the limit");
        assert_eq!(err.requested, 1);
        assert_eq!((r.bytes(), c.in_use()), (100, 100));
    }

    #[test]
    fn grow_refuses_an_overflowing_size_unchanged() {
        let c = counter(usize::MAX);
        let mut r = Reservation::take(&c, 2).expect("fits");
        let err = r
            .try_grow(usize::MAX)
            .expect_err("overflows the reservation");
        assert_eq!(
            (err.requested, err.in_use, err.limit),
            (usize::MAX, 2, usize::MAX)
        );
        assert_eq!((r.bytes(), c.in_use()), (2, 2));
    }

    #[test]
    fn shrink_releases_at_once_and_clamps_to_what_is_held() {
        let c = counter(100);
        let mut r = Reservation::take(&c, 80).expect("fits");
        r.shrink(30);
        assert_eq!((r.bytes(), c.in_use()), (50, 50));
        r.shrink(1_000);
        assert_eq!((r.bytes(), c.in_use()), (0, 0));
        drop(r);
        assert_eq!(c.in_use(), 0);
    }

    #[test]
    fn drop_returns_the_bytes() {
        let c = counter(100);
        let r = Reservation::take(&c, 70).expect("fits");
        assert_eq!(c.in_use(), 70);
        drop(r);
        assert_eq!(c.in_use(), 0);
        assert_eq!(c.limit(), 100);
    }
}

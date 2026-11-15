//! The node memory budget: a total split into named pools.

use std::sync::Arc;

use crate::reservation::PoolCounter;
use crate::{ConfigError, Exhausted, Pool, Reservation};

/// How a node's memory is split into pools, checked to fit the node total.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetConfig {
    total: usize,
    pools: [usize; Pool::ALL.len()],
}

impl BudgetConfig {
    /// A budget of `total` bytes with every pool at zero.
    pub const fn new(total: usize) -> Self {
        Self {
            total,
            pools: [0; Pool::ALL.len()],
        }
    }

    /// Sets `pool` to `bytes`, refusing a split whose pools no longer fit the
    /// node total.
    pub fn with_pool(mut self, pool: Pool, bytes: usize) -> Result<Self, ConfigError> {
        let mut sizes = self.pools;
        sizes[pool.index()] = bytes;
        let sum = sizes
            .iter()
            .try_fold(0usize, |acc, &b| acc.checked_add(b))
            .ok_or(ConfigError::Overflow)?;
        if sum > self.total {
            return Err(ConfigError::PoolsExceedTotal {
                pools: sum,
                total: self.total,
            });
        }
        self.pools = sizes;
        Ok(self)
    }

    /// The node total in bytes.
    pub const fn total(&self) -> usize {
        self.total
    }

    /// The bytes given to `pool`.
    pub const fn pool(&self, pool: Pool) -> usize {
        self.pools[pool.index()]
    }
}

/// A node's memory budget. Cheap to clone; clones share the same pools.
#[derive(Clone, Debug)]
pub struct MemoryBudget {
    total: usize,
    counters: Arc<[Arc<PoolCounter>; Pool::ALL.len()]>,
}

impl MemoryBudget {
    /// Builds the budget from a checked configuration.
    pub fn new(config: &BudgetConfig) -> Self {
        let counters = Pool::ALL.map(|p| Arc::new(PoolCounter::new(p, config.pool(p))));
        Self {
            total: config.total(),
            counters: Arc::new(counters),
        }
    }

    /// Reserves `bytes` from `pool`, or refuses without changing the pool.
    ///
    /// Call before allocating. The reservation is released when dropped.
    pub fn try_reserve(&self, pool: Pool, bytes: usize) -> Result<Reservation, Exhausted> {
        Reservation::take(self.counter(pool), bytes)
    }

    /// The node total in bytes.
    pub fn total(&self) -> usize {
        self.total
    }

    /// `pool`'s limit in bytes.
    pub fn limit(&self, pool: Pool) -> usize {
        self.counter(pool).limit()
    }

    /// Bytes currently reserved from `pool`.
    pub fn in_use(&self, pool: Pool) -> usize {
        self.counter(pool).in_use()
    }

    /// Bytes `pool` can still grant.
    pub fn available(&self, pool: Pool) -> usize {
        self.limit(pool).saturating_sub(self.in_use(pool))
    }

    fn counter(&self, pool: Pool) -> &Arc<PoolCounter> {
        &self.counters[pool.index()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> MemoryBudget {
        let config = BudgetConfig::new(1_000)
            .with_pool(Pool::RequestBodies, 300)
            .and_then(|c| c.with_pool(Pool::QueryExecution, 200))
            .expect("fits");
        MemoryBudget::new(&config)
    }

    #[test]
    fn pools_summing_to_the_total_are_accepted_one_more_byte_refused() {
        let at = BudgetConfig::new(100)
            .with_pool(Pool::PageCache, 60)
            .and_then(|c| c.with_pool(Pool::Transport, 40))
            .expect("pools sum to exactly the total");
        assert_eq!(
            (
                at.total(),
                at.pool(Pool::PageCache),
                at.pool(Pool::Transport)
            ),
            (100, 60, 40)
        );
        let err = at
            .with_pool(Pool::Transport, 41)
            .expect_err("one byte over");
        assert_eq!(
            err,
            ConfigError::PoolsExceedTotal {
                pools: 101,
                total: 100
            }
        );
    }

    #[test]
    fn resizing_a_pool_replaces_its_size() {
        let c = BudgetConfig::new(100)
            .with_pool(Pool::PageCache, 90)
            .and_then(|c| c.with_pool(Pool::PageCache, 100))
            .expect("replaced, not added");
        assert_eq!(c.pool(Pool::PageCache), 100);
    }

    #[test]
    fn overflowing_pool_sizes_are_refused() {
        let c = BudgetConfig::new(usize::MAX)
            .with_pool(Pool::PageCache, usize::MAX)
            .expect("fits");
        assert_eq!(c.with_pool(Pool::Transport, 1), Err(ConfigError::Overflow));
    }

    #[test]
    fn reserve_exactly_free_bytes_then_refuse_one_more() {
        let b = budget();
        let _a = b.try_reserve(Pool::RequestBodies, 100).expect("fits");
        let _rest = b
            .try_reserve(Pool::RequestBodies, 200)
            .expect("exactly the free bytes");
        let err = b.try_reserve(Pool::RequestBodies, 1).expect_err("one past");
        assert_eq!(
            err,
            Exhausted {
                pool: Pool::RequestBodies,
                requested: 1,
                in_use: 300,
                limit: 300
            }
        );
    }

    #[test]
    fn pools_are_independent_and_drop_returns_bytes() {
        let b = budget();
        let full = b.try_reserve(Pool::RequestBodies, 300).expect("fits");
        assert_eq!(b.available(Pool::RequestBodies), 0);
        assert_eq!(b.available(Pool::QueryExecution), 200);
        let q = b
            .try_reserve(Pool::QueryExecution, 200)
            .expect("other pool unaffected");
        drop(full);
        assert_eq!(
            (
                b.in_use(Pool::RequestBodies),
                b.available(Pool::RequestBodies)
            ),
            (0, 300)
        );
        assert_eq!(b.in_use(Pool::QueryExecution), 200);
        drop(q);
        assert_eq!(b.in_use(Pool::QueryExecution), 0);
    }

    #[test]
    fn unconfigured_pools_grant_nothing() {
        let b = budget();
        assert_eq!(b.limit(Pool::Transport), 0);
        assert!(b.try_reserve(Pool::Transport, 1).is_err());
        assert_eq!(b.total(), 1_000);
    }

    #[test]
    fn clones_share_pools() {
        let a = budget();
        let b = a.clone();
        let _r = a.try_reserve(Pool::RequestBodies, 300).expect("fits");
        assert!(b.try_reserve(Pool::RequestBodies, 1).is_err());
    }
}

//! When a shard's indexing buffer is written out: the node's shards share
//! one budget, and those using most of it are refreshed first.

mod budget;

use std::collections::HashSet;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError};

pub use budget::{BufferSize, DEFAULT_MIN_BYTES, DEFAULT_PERCENT_BASIS_POINTS, IndexingBudget};

/// Core's default for `indices.memory.shard_inactive_time`: five minutes.
pub const DEFAULT_INACTIVE_MILLIS: u64 = 300_000;

/// What one shard is using, as the node reads it for a check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShardUsage<K> {
    /// The shard.
    pub key: K,
    /// The heap its indexing buffer uses.
    pub buffer_bytes: u64,
    /// The bytes it is already writing out.
    pub writing_bytes: u64,
    /// The time since its last write.
    pub idle_millis: u64,
}

/// What a check asks the node to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision<K> {
    /// Shards with no write for the inactive time: flush them.
    pub flush_idle: Vec<K>,
    /// Shards to refresh, largest first.
    pub refresh: Vec<K>,
    /// Shards to throttle to one writer.
    pub throttle: Vec<K>,
    /// Shards that were throttled and are released.
    pub release: Vec<K>,
}

impl<K> Default for Decision<K> {
    fn default() -> Self {
        Self {
            // ALLOC: empty lists; they grow to at most the node's shards.
            flush_idle: Vec::new(),
            refresh: Vec::new(),
            throttle: Vec::new(),
            // ALLOC: empty list, as above.
            release: Vec::new(),
        }
    }
}

/// The controller of one node's indexing buffers.
pub struct IndexingMemory<K> {
    budget: IndexingBudget,
    inactive_millis: u64,
    since_check: AtomicU64,
    running: Mutex<()>,
    throttled: Mutex<HashSet<K>>,
}

impl<K> std::fmt::Debug for IndexingMemory<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexingMemory")
            .field("budget", &self.budget)
            .field("inactive_millis", &self.inactive_millis)
            .finish_non_exhaustive()
    }
}

/// Counts and a set of keys stay consistent across a panic, so a poisoned
/// lock is recovered.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl<K: Clone + Eq + Hash> IndexingMemory<K> {
    /// A controller over `budget`, flushing shards idle for `inactive_millis`.
    pub fn new(budget: IndexingBudget, inactive_millis: u64) -> Self {
        Self {
            budget,
            inactive_millis,
            since_check: AtomicU64::new(0),
            running: Mutex::new(()),
            // ALLOC: the shards being throttled, at most the node's shards.
            throttled: Mutex::new(HashSet::new()),
        }
    }

    /// The budget.
    pub fn budget(&self) -> IndexingBudget {
        self.budget
    }

    /// The bytes written since the last check was asked for.
    pub fn pending_bytes(&self) -> u64 {
        self.since_check.load(Ordering::SeqCst)
    }

    /// Records `bytes` written (the estimated size of one operation), and
    /// says whether a check is due: the bytes since the last one passed a
    /// thirtieth of the budget. Of callers racing at that point one is told,
    /// and the count drops by what it found.
    pub fn bytes_written(&self, bytes: u64) -> bool {
        let interval = self.budget.check_interval_bytes();
        let total = self
            .since_check
            .fetch_add(bytes, Ordering::SeqCst)
            .saturating_add(bytes);
        if total <= interval {
            return false;
        }
        let _turn = match self.running.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            // Another caller is deciding: it does the work.
            Err(TryLockError::WouldBlock) => return false,
        };
        // Pulled again: it may have moved since the first look.
        let now = self.since_check.load(Ordering::SeqCst);
        if now <= interval {
            return false;
        }
        self.since_check.fetch_sub(now, Ordering::SeqCst);
        true
    }

    /// What to do about `shards` now.
    pub fn check(&self, shards: &[ShardUsage<K>]) -> Decision<K> {
        let _turn = lock(&self.running);
        let mut decision = Decision::default();
        // ALLOC: one entry per shard the node gave, at most its shards.
        let mut counted: Vec<(u64, &K)> = Vec::new();
        let mut total_used = 0u64;
        let mut total_writing = 0u64;
        for shard in shards {
            if shard.idle_millis >= self.inactive_millis {
                // ALLOC: the shards to flush, at most the node's shards.
                decision.flush_idle.push(shard.key.clone());
            }
            // Bytes already moving to disk are not counted twice, and a
            // refresh that finished between the two readings counts as none.
            let used = shard.buffer_bytes.saturating_sub(shard.writing_bytes);
            total_writing = total_writing.saturating_add(shard.writing_bytes);
            total_used = total_used.saturating_add(used);
            // Shards using nothing sort last and are never reached: the loop
            // below stops once the rest fits.
            counted.push((used, &shard.key));
        }
        let throttling = self
            .budget
            .past_one_and_a_half(total_writing.saturating_add(total_used));
        let mut throttled = lock(&self.throttled);
        // Largest first; the sort is stable, so ties keep the node's order.
        counted.sort_by_key(|(used, _)| std::cmp::Reverse(*used));
        for (used, key) in counted {
            if total_used <= self.budget.bytes() {
                break;
            }
            // ALLOC: the shards to refresh, at most the node's shards.
            decision.refresh.push(key.clone());
            total_used = total_used.saturating_sub(used);
            if throttling && throttled.insert(key.clone()) {
                // ALLOC: the shards to throttle, at most the node's shards.
                decision.throttle.push(key.clone());
            }
        }
        if !throttling {
            decision.release.extend(throttled.drain());
        }
        decision
    }
}

//! The resolutions a node's routing may answer from: a name, the register
//! it was read from, and when.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/RoutingCache.java`.
//! A descriptor read is the whole store cost of a warm search and half of
//! a write's: one GET a request for each index it names, for a fact that
//! changes when an operator changes it. Routing alone may answer from a
//! read up to a window old (a second by default). Every mutating caller
//! still reads through, because a compare-and-swap needs the generation it
//! read to be current.
//!
//! A stale answer is bounded:
//! - A miss is never cached, so an index is routable the moment it exists.
//! - A change this node makes is forgotten at once, through
//!   [`DescriptorStore::on_changed`], so this node reads its own writes.
//! - Another node's change shows within the window.
//!
//! The landing shard checks the index's uuid again, which catches a stale
//! answer only when the landing node no longer holds the old incarnation's
//! shard. When it still does, both uuids are the old one: what closes that
//! is the landing node's incarnation fence, which is not ported yet. Until
//! it is, writes must not route through this cache.
//!
//! The cache keeps each register's bytes, so its memory is the bytes it
//! holds, under a byte budget. A hit copies them into the caller's buffer
//! and parses them there, with no request.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use crate::descriptors::{DescriptorStore, Resolution, Target, published, resolution};
use crate::error::MetaError;

/// How long routing may answer from an earlier read
/// (`DEFAULT_ROUTING_CACHE_MILLIS`): the interval membership is probed on.
pub const DEFAULT_ROUTING_CACHE_MILLIS: i64 = 1_000;

/// The bytes the cache may hold (`DEFAULT_ROUTING_CACHE_MAXIMUM_WEIGHT`).
pub const DEFAULT_ROUTING_CACHE_BYTES: usize = 64 << 20;

/// Past this many names, entries older than the window are dropped, at
/// most once a window (`DEFAULT_ROUTING_CACHE_PRUNE_ABOVE`).
pub const DEFAULT_ROUTING_CACHE_PRUNE_ABOVE: usize = 10_000;

/// Bytes an entry costs beyond its register: the name, the map's slot and
/// the entry's own fields.
const ENTRY_OVERHEAD: usize = 128;

/// A register as read, and when.
struct Entry {
    generation: u64,
    value: Box<[u8]>,
    at_millis: i64,
}

impl Entry {
    fn weight(&self, name: &str) -> usize {
        self.value
            .len()
            .saturating_add(name.len())
            .saturating_add(ENTRY_OVERHEAD)
    }
}

struct Inner {
    entries: HashMap<String, Entry>,
    bytes: usize,
    // Moves on every local change, so a read that began before one does
    // not put back what the change made stale.
    epoch: u64,
    pruned_at: Option<i64>,
    // No entry was read before this, so a walk before it is a window old
    // finds nothing to drop. It may be older than the oldest entry left.
    oldest: Option<i64>,
    // Walks over the whole map, for the test that pins how often they run.
    walks: u64,
    window_millis: i64,
    budget_bytes: usize,
    prune_above: usize,
}

impl Inner {
    fn remove(&mut self, name: &str) {
        if let Some(gone) = self.entries.remove(name) {
            self.bytes = self.bytes.saturating_sub(gone.weight(name));
        }
    }

    /// Drops every entry past the window.
    fn drop_expired(&mut self, now_millis: i64) {
        let window = self.window_millis;
        if self
            .oldest
            .is_none_or(|oldest| now_millis.saturating_sub(oldest) <= window)
        {
            return;
        }
        self.walks = self.walks.saturating_add(1);
        let mut oldest = None::<i64>;
        let mut freed = 0usize;
        self.entries.retain(|name, entry| {
            let keep = now_millis.saturating_sub(entry.at_millis) <= window;
            if keep {
                oldest = Some(oldest.map_or(entry.at_millis, |o| o.min(entry.at_millis)));
            } else {
                freed = freed.saturating_add(entry.weight(name));
            }
            keep
        });
        self.bytes = self.bytes.saturating_sub(freed);
        self.oldest = oldest;
        // A table sized for a past peak would hold memory the budget does
        // not count.
        if self.entries.len() < self.entries.capacity() / 4 {
            self.entries.shrink_to(self.entries.len().saturating_mul(2));
        }
    }

    /// Past the entry threshold, drops expired entries, at most once a
    /// window: a walk on every insert would make a busy node pay for its
    /// own cache.
    fn prune(&mut self, now_millis: i64) {
        if self.entries.len() <= self.prune_above
            || self
                .pruned_at
                .is_some_and(|at| now_millis.saturating_sub(at) < self.window_millis)
        {
            return;
        }
        self.pruned_at = Some(now_millis);
        self.drop_expired(now_millis);
    }
}

/// A node's routing resolutions, shared by its request threads.
pub struct RoutingCache {
    inner: Mutex<Inner>,
}

impl Default for RoutingCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RoutingCache {
    /// An empty cache with the Java's defaults.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                // ALLOC: none until a name is cached; entries are bounded by
                // the byte budget.
                entries: HashMap::new(),
                bytes: 0,
                epoch: 0,
                pruned_at: None,
                oldest: None,
                walks: 0,
                window_millis: DEFAULT_ROUTING_CACHE_MILLIS,
                budget_bytes: DEFAULT_ROUTING_CACHE_BYTES,
                prune_above: DEFAULT_ROUTING_CACHE_PRUNE_ABOVE,
            }),
        }
    }

    /// The lock, kept usable after a panic elsewhere: every update leaves
    /// the map consistent, and the worst a lost one does is a read through.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Sets how long routing may answer from an earlier read; 0 or less
    /// reads through every time.
    pub fn set_window_millis(&self, millis: i64) {
        self.lock().window_millis = millis.max(0);
    }

    /// Sets the bytes the cache may hold; a register longer than that is
    /// not kept. A budget below what the cache holds empties it.
    pub fn set_budget_bytes(&self, bytes: usize) {
        let mut inner = self.lock();
        inner.budget_bytes = bytes;
        if inner.bytes > bytes {
            // ALLOC: none: an empty map, giving back the old table.
            inner.entries = HashMap::new();
            inner.oldest = None;
            inner.bytes = 0;
        }
    }

    /// Sets how many names the cache may hold before expired ones are
    /// dropped.
    pub fn set_prune_above(&self, entries: usize) {
        self.lock().prune_above = entries;
    }

    /// How many names the cache holds, and their bytes.
    pub fn size(&self) -> (usize, usize) {
        let inner = self.lock();
        (inner.entries.len(), inner.bytes)
    }

    /// Forgets `name`, so a change this node just made routes at once;
    /// what [`DescriptorStore::on_changed`] calls. Only this node's cache:
    /// another node's ages out within its window.
    pub fn forget(&self, name: &str) {
        let mut inner = self.lock();
        inner.epoch = inner.epoch.wrapping_add(1);
        inner.remove(name);
    }

    /// Resolves `name` for routing, as [`DescriptorStore::resolve`] does,
    /// but from a read up to the window old: no request then. A miss, or an
    /// absent name, reads through, and an absent one is not kept. The
    /// record borrows from `buf`.
    ///
    /// # Errors
    ///
    /// As [`DescriptorStore::resolve`]; a cached register longer than `buf`
    /// is read through, and refused as the store refuses it.
    pub fn resolve<'b>(
        &self,
        descriptors: &DescriptorStore,
        name: &str,
        now_millis: i64,
        buf: &'b mut [u8],
    ) -> Result<Resolution<'b>, MetaError> {
        let epoch = {
            let inner = self.lock();
            if inner.window_millis <= 0 {
                drop(inner);
                return descriptors.resolve(name, buf);
            }
            if let Some(entry) = inner.entries.get(name)
                && (0..=inner.window_millis).contains(&now_millis.saturating_sub(entry.at_millis))
                && let Some(room) = buf.get_mut(..entry.value.len())
            {
                room.copy_from_slice(&entry.value);
                let generation = entry.generation;
                let len = entry.value.len();
                drop(inner);
                let buf: &'b [u8] = buf;
                let value = buf.get(..len).unwrap_or_default();
                return Ok(published(resolution(generation, value)?));
            }
            inner.epoch
        };
        let Some((generation, len)) = descriptors.read_value(name, buf)? else {
            self.forget_if(name, epoch);
            return Ok(Resolution {
                generation: oss_blob::register::ABSENT_GENERATION,
                target: Target::Absent,
            });
        };
        let buf: &'b [u8] = buf;
        let value = buf.get(..len).unwrap_or_default();
        // Parsed first: a malformed register is an error, never cached.
        let resolved = published(resolution(generation, value)?);
        if matches!(resolved.target, Target::Absent) {
            self.forget_if(name, epoch);
        } else {
            self.keep(name, epoch, generation, value, now_millis);
        }
        Ok(resolved)
    }

    /// Drops `name` unless a local change came first, which already did.
    fn forget_if(&self, name: &str, epoch: u64) {
        let mut inner = self.lock();
        if inner.epoch == epoch {
            inner.remove(name);
        }
    }

    /// Keeps a fresh read, unless a local change began after the read did,
    /// or it does not fit the budget once expired entries are dropped.
    fn keep(&self, name: &str, epoch: u64, generation: u64, value: &[u8], now_millis: i64) {
        let mut inner = self.lock();
        if inner.epoch != epoch {
            return;
        }
        inner.remove(name);
        let entry = Entry {
            generation,
            // ALLOC: one register's bytes, within the cache's byte budget.
            value: value.into(),
            at_millis: now_millis,
        };
        let weight = entry.weight(name);
        if inner.bytes.saturating_add(weight) > inner.budget_bytes {
            inner.drop_expired(now_millis);
        }
        // A register that still does not fit is not kept: one large
        // mapping must not evict every small resolution.
        if inner.bytes.saturating_add(weight) <= inner.budget_bytes {
            inner.bytes = inner.bytes.saturating_add(weight);
            inner.oldest = Some(inner.oldest.map_or(now_millis, |o| o.min(now_millis)));
            // ALLOC: one name a cached register, within the byte budget.
            inner.entries.insert(name.to_owned(), entry);
        }
        inner.prune(now_millis);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(at_millis: i64) -> Entry {
        Entry {
            generation: 1,
            value: Box::new([0u8; 16]),
            at_millis,
        }
    }

    #[test]
    fn a_full_cache_of_fresh_entries_is_not_walked() {
        let cache = RoutingCache::new();
        let mut inner = cache.lock();
        for (i, at) in [0, 10].into_iter().enumerate() {
            let e = entry(at);
            inner.bytes += e.weight("n");
            inner.entries.insert(format!("n{i}"), e);
        }
        inner.oldest = Some(0);
        // Nothing can have expired within a window of the oldest read.
        inner.drop_expired(DEFAULT_ROUTING_CACHE_MILLIS);
        assert_eq!((inner.walks, inner.entries.len()), (0, 2));
        // Past it, one walk drops the expired entry, and the next oldest
        // is remembered.
        inner.drop_expired(DEFAULT_ROUTING_CACHE_MILLIS + 1);
        assert_eq!((inner.walks, inner.entries.len()), (1, 1));
        assert_eq!(inner.oldest, Some(10));
        inner.drop_expired(DEFAULT_ROUTING_CACHE_MILLIS + 10);
        assert_eq!(inner.walks, 1);
        // An empty cache has nothing to walk.
        inner.drop_expired(i64::MAX);
        assert_eq!(
            (inner.walks, inner.entries.len(), inner.oldest),
            (2, 0, None)
        );
        inner.drop_expired(i64::MAX);
        assert_eq!(inner.walks, 2);
    }

    #[test]
    fn a_walk_gives_back_a_table_sized_for_a_past_peak() {
        let cache = RoutingCache::new();
        let mut inner = cache.lock();
        for i in 0..64 {
            inner.entries.insert(format!("n{i}"), entry(0));
        }
        inner.entries.insert("fresh".to_owned(), entry(5_000));
        inner.oldest = Some(0);
        let peak = inner.entries.capacity();
        inner.drop_expired(5_000);
        assert_eq!(inner.entries.len(), 1);
        assert!(
            inner.entries.capacity() < peak / 4,
            "{}",
            inner.entries.capacity()
        );
    }

    #[test]
    fn a_table_a_quarter_full_is_kept() {
        let cache = RoutingCache::new();
        let mut inner = cache.lock();
        for i in 0..64 {
            inner.entries.insert(format!("n{i}"), entry(0));
        }
        let capacity = inner.entries.capacity();
        // Exactly a quarter of the table stays fresh.
        for i in 0..capacity / 4 {
            inner.entries.insert(format!("n{i}"), entry(5_000));
        }
        inner.oldest = Some(0);
        inner.drop_expired(5_000);
        assert_eq!(inner.entries.len(), capacity / 4);
        // Not shrunk, which would halve it; removals may cost a few slots.
        assert!(
            inner.entries.capacity() > capacity / 2,
            "{}",
            inner.entries.capacity()
        );
    }
}

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
//! The landing shard checks the index's uuid again, which is what keeps a
//! stale answer from acknowledging into a deleted index.
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
        let mut freed = 0usize;
        self.entries.retain(|name, entry| {
            let keep = now_millis.saturating_sub(entry.at_millis) <= window;
            if !keep {
                freed = freed.saturating_add(entry.weight(name));
            }
            keep
        });
        self.bytes = self.bytes.saturating_sub(freed);
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
    /// not kept. Entries already held over it are dropped.
    pub fn set_budget_bytes(&self, bytes: usize) {
        let mut inner = self.lock();
        inner.budget_bytes = bytes;
        if inner.bytes > bytes {
            inner.entries.clear();
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
                && now_millis.saturating_sub(entry.at_millis) <= inner.window_millis
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
            // ALLOC: one name a cached register, within the byte budget.
            inner.entries.insert(name.to_owned(), entry);
        }
        inner.prune(now_millis);
    }
}

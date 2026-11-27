//! A bounded cache of what the node computes from a Painless source, so a
//! repeated script is parsed and checked once and not on every request. See
//! docs/specs/painless-compile-cache.
//!
//! Process-wide, as the switch is, and on only after [`install`] reserved
//! its bytes from the node's query-execution pool.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Entries the cache holds at most.
const MAX_ENTRIES: usize = 256;

/// Bytes the cache holds at most; the node reserves them from its pool.
pub const CACHE_BYTES: usize = 4 << 20;

#[derive(Default)]
struct State {
    map: HashMap<Vec<u8>, Arc<[u8]>>,
    order: VecDeque<Vec<u8>>,
    bytes: usize,
    hits: u64,
    misses: u64,
}

/// A bounded map from a key to bytes, oldest out.
pub(crate) struct Cache {
    max_entries: usize,
    max_bytes: usize,
    state: Mutex<State>,
}

fn charge(key: &[u8], value: &[u8]) -> usize {
    key.len().saturating_add(value.len())
}

impl Cache {
    pub(crate) fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            max_entries,
            max_bytes,
            state: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The value for `key`, computed by `compute` and kept when it is not
    /// there. An error is returned and not kept; a value that cannot fit the
    /// budget is returned and not kept.
    pub(crate) fn get_or_compute<E>(
        &self,
        key: &[u8],
        compute: impl FnOnce() -> Result<Vec<u8>, E>,
    ) -> Result<Arc<[u8]>, E> {
        {
            let mut s = self.lock();
            if let Some(v) = s.map.get(key) {
                let v = Arc::clone(v);
                s.hits = s.hits.saturating_add(1);
                return Ok(v);
            }
            s.misses = s.misses.saturating_add(1);
        }
        // Computed outside the lock: a compile is the slow part.
        let value: Arc<[u8]> = Arc::from(compute()?);
        let size = charge(key, &value);
        if size > self.max_bytes {
            return Ok(value);
        }
        let mut s = self.lock();
        if s.map.contains_key(key) {
            return Ok(value);
        }
        while !s.order.is_empty()
            && (s.map.len() >= self.max_entries || s.bytes.saturating_add(size) > self.max_bytes)
        {
            if let Some((old, v)) = s
                .order
                .pop_front()
                .and_then(|old| s.map.remove(&old).map(|v| (old, v)))
            {
                s.bytes = s.bytes.saturating_sub(charge(&old, &v));
            }
        }
        // ALLOC: one key and one value, within the cache's byte budget.
        s.map.insert(key.to_vec(), Arc::clone(&value));
        s.order.push_back(key.to_vec());
        s.bytes = s.bytes.saturating_add(size);
        Ok(value)
    }

    /// Hits and misses so far.
    #[cfg(test)]
    pub(crate) fn counts(&self) -> (u64, u64) {
        let s = self.lock();
        (s.hits, s.misses)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().map.len()
    }

    /// Bytes charged now.
    pub(crate) fn bytes(&self) -> usize {
        self.lock().bytes
    }
}

/// The cache and whatever pays for its bytes, kept while it is on.
pub(crate) struct Installed {
    cache: Cache,
    _payment: Box<dyn Send + Sync>,
}

impl Installed {
    pub(crate) fn new(payment: Box<dyn Send + Sync>, bytes: usize) -> Self {
        Self {
            cache: Cache::new(MAX_ENTRIES, bytes),
            _payment: payment,
        }
    }
}

static INSTALLED: Mutex<Option<Arc<Installed>>> = Mutex::new(None);

/// Turns the cache on, holding `payment` (the node's reservation of
/// [`CACHE_BYTES`] from its pool) until it is removed.
pub fn install(payment: Box<dyn Send + Sync>) {
    let installed = Arc::new(Installed::new(payment, CACHE_BYTES));
    *INSTALLED.lock().unwrap_or_else(PoisonError::into_inner) = Some(installed);
}

/// Turns the cache off and returns its bytes to the pool.
pub fn remove() {
    *INSTALLED.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Bytes the installed cache holds now; zero when it is off.
pub fn bytes() -> usize {
    INSTALLED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map_or(0, |i| i.cache.bytes())
}

/// The value for `key` from the installed cache, or from `compute` when the
/// cache is off.
pub(crate) fn cached<E>(
    key: &[u8],
    compute: impl FnOnce() -> Result<Vec<u8>, E>,
) -> Result<Arc<[u8]>, E> {
    let installed = INSTALLED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    match installed {
        Some(i) => i.cache.get_or_compute(key, compute),
        None => compute().map(Arc::from),
    }
}

#[cfg(test)]
#[path = "painless_cache_tests.rs"]
mod tests;

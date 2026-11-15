//! What a store last read or wrote for each register: its generation and
//! ETag, so a swap against that very version is one conditional PUT
//! rather than a GET and a PUT.
//!
//! Ported from `S3BlobContainer.knownVersions`
//! (`plugins/repository-s3/src/main/java/org/opensearch/repositories/s3/S3BlobContainer.java@3920dc36`).
//! The ETag is a safe token for a generation because no two versions of a
//! register have the same bytes: a stale entry costs a refused PUT and a
//! read, never a wrong write. Bounded: an entry that falls out costs the
//! GET it would have saved.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The most registers remembered, as the Java's.
pub(crate) const MAX_KNOWN: usize = 10_000;

/// One remembered version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Known {
    pub(crate) generation: u64,
    pub(crate) etag: String,
}

#[derive(Debug, Default)]
struct Inner {
    /// Each key's version and the sequence number it was added under.
    map: HashMap<Arc<str>, (Known, u64)>,
    /// Keys with the sequence number they were added under, oldest first;
    /// the oldest is forgotten first. A position whose number no longer
    /// matches the map's (the key was forgotten since) is skipped.
    order: VecDeque<(u64, Arc<str>)>,
    next: u64,
}

/// The remembered versions of at most `cap` registers.
#[derive(Debug)]
pub(crate) struct KnownVersions {
    inner: Mutex<Inner>,
    cap: usize,
}

impl Default for KnownVersions {
    fn default() -> Self {
        Self::with_cap(MAX_KNOWN)
    }
}

impl KnownVersions {
    /// Remembers at most `cap` registers.
    pub(crate) fn with_cap(cap: usize) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            cap,
        }
    }

    /// Locks the map, recovering it if a panicking thread poisoned it:
    /// each update is a single insert or remove, so it stays consistent,
    /// and a wrong entry only costs a refused write.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The version remembered for `key`, if any.
    pub(crate) fn get(&self, key: &str) -> Option<Known> {
        // ALLOC: one ETag, as long as the store's header value.
        self.lock().map.get(key).map(|(known, _)| known.clone())
    }

    /// Remembers that `key` is at `generation` with `etag`.
    pub(crate) fn remember(&self, key: &str, generation: u64, etag: &str) {
        let mut inner = self.lock();
        // ALLOC: one ETag, as long as the store's header value.
        let known = Known {
            generation,
            etag: etag.to_owned(),
        };
        if let Some((slot, _)) = inner.map.get_mut(key) {
            *slot = known;
            return;
        }
        let seq = inner.next;
        inner.next = seq.wrapping_add(1);
        // ALLOC: one key, at most MAX_KEY_BYTES, for each of at most MAX_KNOWN
        // registers (about 11 MiB at worst), shared by the map and the queue.
        let key: Arc<str> = Arc::from(key);
        inner.map.insert(Arc::clone(&key), (known, seq));
        // ALLOC: the queue holds at most twice MAX_KNOWN handles, see below.
        inner.order.push_back((seq, key));
        while inner.map.len() > self.cap {
            let Some((seq, oldest)) = inner.order.pop_front() else {
                break;
            };
            if inner.map.get(&oldest).is_some_and(|(_, s)| *s == seq) {
                inner.map.remove(&oldest);
            }
        }
        // Positions of keys forgotten since pile up only until the queue is
        // twice the map's cap; then the ones still current are kept.
        if inner.order.len() > self.cap.saturating_mul(2) {
            let Inner { map, order, .. } = &mut *inner;
            order.retain(|(seq, k)| map.get(k).is_some_and(|(_, s)| s == seq));
        }
    }

    /// Forgets `key`.
    pub(crate) fn forget(&self, key: &str) {
        self.lock().map.remove(key);
    }

    /// How many registers are remembered.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_updates_and_forgets() {
        let k = KnownVersions::default();
        assert_eq!(k.get("a"), None);
        k.remember("a", 5, "e1");
        assert_eq!(
            k.get("a"),
            Some(Known {
                generation: 5,
                etag: "e1".into()
            })
        );
        k.remember("a", 6, "e2");
        assert_eq!(k.get("a").unwrap().generation, 6);
        k.forget("a");
        assert_eq!(k.get("a"), None);
    }

    #[test]
    fn holds_at_most_its_cap_forgetting_the_oldest() {
        let k = KnownVersions::with_cap(3);
        for (i, key) in ["a", "b", "c", "d"].iter().enumerate() {
            k.remember(key, i as u64, "e");
        }
        assert_eq!(k.len(), 3);
        assert_eq!(k.get("a"), None);
        assert!(k.get("d").is_some());
        // A key forgotten and remembered again leaves a stale position,
        // which neither evicts it early nor grows the queue past its bound.
        // "b" is now the newest: "c" goes first.
        for _ in 0..100 {
            k.forget("b");
            k.remember("b", 1, "e");
        }
        assert!(k.lock().order.len() <= 7);
        k.remember("e", 9, "e");
        assert!(k.get("b").is_some());
        assert_eq!(k.get("c"), None);
        assert_eq!(k.len(), 3);
    }
}

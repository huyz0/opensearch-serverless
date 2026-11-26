//! What serialises publishes: one node-wide gate, or one gate a shard when
//! packs are on and shards publish together.

use std::collections::HashSet;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

use super::publishing::Key;

/// The gates a [`super::Publishing`] holds.
#[derive(Default)]
pub(super) struct Gates {
    wide: Mutex<()>,
    busy: Mutex<HashSet<Key>>,
    freed: Condvar,
}

/// Held from taking a shard's snapshot to publishing it.
pub(super) enum Gate<'a> {
    /// The node-wide gate: no other shard publishes meanwhile.
    Wide {
        /// Held until the gate drops.
        _held: MutexGuard<'a, ()>,
    },
    /// One shard's gate: other shards publish meanwhile.
    Shard {
        /// Released when the gate drops.
        _held: ShardGate<'a>,
    },
}

/// A shard's gate, released when dropped.
pub(super) struct ShardGate<'a> {
    gates: &'a Gates,
    key: Key,
}

impl Gates {
    /// The gate for `key`: the shard's own when `per_shard`, else the one
    /// every shard shares. Waits for it.
    pub(super) fn take(&self, key: &Key, per_shard: bool) -> Gate<'_> {
        if !per_shard {
            return Gate::Wide {
                _held: self.wide.lock().unwrap_or_else(PoisonError::into_inner),
            };
        }
        let mut busy = self.busy.lock().unwrap_or_else(PoisonError::into_inner);
        while busy.contains(key) {
            busy = self
                .freed
                .wait(busy)
                .unwrap_or_else(PoisonError::into_inner);
        }
        // ALLOC: one key a shard publishing now.
        busy.insert(key.clone());
        Gate::Shard {
            _held: ShardGate {
                gates: self,
                key: key.clone(),
            },
        }
    }
}

impl Drop for ShardGate<'_> {
    fn drop(&mut self) {
        let mut busy = self
            .gates
            .busy
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        busy.remove(&self.key);
        self.gates.freed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn most_in_flight(keys: [Key; 2], per_shard: bool) -> usize {
        let gates = Gates::default();
        let now = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for key in &keys {
                s.spawn(|| {
                    let _gate = gates.take(key, per_shard);
                    let n = now.fetch_add(1, Ordering::SeqCst).saturating_add(1);
                    most.fetch_max(n, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(100));
                    now.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        most.load(Ordering::SeqCst)
    }

    fn key(n: u32) -> Key {
        ("u".to_owned(), n)
    }

    // AC-4
    #[test]
    fn a_shard_never_has_two_publishes_in_flight() {
        assert_eq!(most_in_flight([key(0), key(0)], true), 1);
    }

    // AC-2: different shards overlap with per-shard gates, and not with the wide one.
    #[test]
    fn different_shards_overlap_only_with_per_shard_gates() {
        assert_eq!(most_in_flight([key(0), key(1)], true), 2);
        assert_eq!(most_in_flight([key(0), key(1)], false), 1);
    }
}

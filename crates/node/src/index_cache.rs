//! What a writer node knows of its indexes, confirmed by the change stripes.
//!
//! A bulk names an index and the node needs its [`IndexInfo`]; reading the
//! descriptor register costs a request a bulk. This keeps what it read and
//! confirms it by reading the stripe the name hashes to, at most once a
//! window for every index of that stripe, so the requests scale with the
//! stripes ([`STRIPES`]) a node touches and the window, not with indexes or
//! bulks. A stripe that names no change to an index since the descriptor
//! was read proves it unchanged ([`Snapshot::unchanged_since`]); otherwise
//! the next lookup reads the descriptor again. An absent name, an alias and
//! a tombstone are never kept, so a create is seen at once; an update or a
//! delete is seen within one window.
//!
//! Every node of a deployment must record descriptor changes in the stripes
//! (`DescriptorStore::record_changes_in`) or a change goes unseen here.
//!
//! Ported from the routing cache and its confirmation in `ServerlessIndexResolver`.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use oss_meta::{ChangeStripes, STRIPES, Snapshot};
use oss_writer::bulk::IndexInfo;

/// The most indexes kept for each stripe.
const MAX_PER_STRIPE: usize = 8_192;

/// What a lookup found.
pub enum Lookup {
    /// The index, confirmed within the window.
    Hit(IndexInfo),
    /// Nothing kept: read the descriptor and hand what it says to
    /// [`IndexCache::keep`] with this stamp, which was taken before.
    Miss(Stamp),
    /// The stripe could not be read: read the descriptor and keep nothing.
    Uncached,
}

/// The stripe's state just before a descriptor is read: a change after it
/// is a change the read may have missed.
#[derive(Debug, Clone, Copy)]
pub struct Stamp {
    epoch: i64,
    generation: i64,
    at: Instant,
}

struct Entry {
    info: IndexInfo,
    stamp: Stamp,
}

struct View {
    snapshot: Snapshot,
    at: Instant,
}

#[derive(Default)]
struct Stripe {
    view: Option<View>,
    entries: HashMap<String, Entry>,
}

/// The indexes a node has read, confirmed by the change stripes.
pub struct IndexCache {
    stripes: ChangeStripes,
    window: Duration,
    now: Box<dyn Fn() -> Instant + Send + Sync>,
    state: Vec<Mutex<Stripe>>,
}

impl IndexCache {
    /// A cache that confirms what it keeps through `stripes` and trusts a
    /// confirmation for `window`, judging time by `now`.
    pub fn new(
        stripes: ChangeStripes,
        window: Duration,
        now: Box<dyn Fn() -> Instant + Send + Sync>,
    ) -> Self {
        Self {
            stripes,
            window,
            now,
            // ALLOC: one state a stripe, once a node.
            state: (0..STRIPES).map(|_| Mutex::default()).collect(),
        }
    }

    /// The index `name` if it is confirmed, else what to read it with.
    pub fn lookup(&self, name: &str) -> Lookup {
        let Some(slot) = self.state.get(ChangeStripes::stripe_of(name)) else {
            return Lookup::Uncached;
        };
        let mut stripe = slot.lock().unwrap_or_else(PoisonError::into_inner);
        let now = (self.now)();
        if let Some(entry) = stripe.entries.get(name)
            && now.saturating_duration_since(entry.stamp.at) < self.window
        {
            return Lookup::Hit(entry.info.clone());
        }
        let Stripe { view, entries } = &mut *stripe;
        let Some(stamp) = self.refresh(view, name, now) else {
            return Lookup::Uncached;
        };
        if let Some(entry) = entries.get_mut(name) {
            let unchanged = view.as_ref().is_some_and(|v| {
                v.snapshot
                    .unchanged_since(name, entry.stamp.epoch, entry.stamp.generation)
            });
            if unchanged {
                // Unchanged through this read: the next confirmation starts
                // from it, so the entry never ages out of the stripe's kept
                // changes.
                entry.stamp = stamp;
                return Lookup::Hit(entry.info.clone());
            }
            entries.remove(name);
        }
        Lookup::Miss(stamp)
    }

    /// Keeps `info`, which a read of the descriptor `name` gave after
    /// `stamp` was taken.
    pub fn keep(&self, name: &str, info: &IndexInfo, stamp: Stamp) {
        let Some(slot) = self.state.get(ChangeStripes::stripe_of(name)) else {
            return;
        };
        let mut stripe = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if stripe.entries.len() >= MAX_PER_STRIPE && !stripe.entries.contains_key(name) {
            return;
        }
        // ALLOC: one entry an index kept, at most MAX_PER_STRIPE a stripe.
        stripe.entries.insert(
            name.to_owned(),
            Entry {
                info: info.clone(),
                stamp,
            },
        );
    }

    /// Reads the stripe again when the snapshot held is a window old, and
    /// stamps what is held. `None` when it cannot be read.
    fn refresh(&self, held: &mut Option<View>, name: &str, now: Instant) -> Option<Stamp> {
        let stale = held
            .as_ref()
            .is_none_or(|v| now.saturating_duration_since(v.at) >= self.window);
        if stale {
            // One read a stripe a window, shared by every index of it.
            let snapshot = self.stripes.read(ChangeStripes::stripe_of(name)).ok()?;
            *held = Some(View { snapshot, at: now });
        }
        held.as_ref().map(|v| Stamp {
            epoch: v.snapshot.epoch,
            generation: v.snapshot.generation,
            at: v.at,
        })
    }
}

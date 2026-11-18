//! Grows an index's mapping for a write that brings fields it does not have
//! yet: durably, in the descriptor, coalesced per index on this node.

mod attempts;
mod error;

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use oss_meta::DescriptorStore;

pub use attempts::{ATTEMPTS, wait_bound_millis};
pub use error::GrowthError;

use crate::mapping::Mapping;

/// What a caller is told: the mapping the descriptor holds after its
/// addition, or why not.
type Answer = Result<Mapping, GrowthError>;

/// One caller's place in the queue, answered by whichever caller grows the
/// mapping for it.
struct Waiter {
    addition: Mapping,
    answer: Arc<Mutex<Option<Answer>>>,
}

/// One index's callers: the queue, and the turn the leader holds.
#[derive(Default)]
struct Group {
    pending: Mutex<Vec<Waiter>>,
    turn: Mutex<()>,
}

/// A group and how many callers are in it.
struct Entry {
    group: Arc<Group>,
    users: usize,
}

/// Sleeps `ms` milliseconds; injected so a test does not.
pub type Sleeper = Arc<dyn Fn(Duration) + Send + Sync>;

/// Grows mappings in the descriptors of one deployment.
pub struct MappingGrowth {
    descriptors: Arc<DescriptorStore>,
    sleep: Sleeper,
    groups: Mutex<HashMap<String, Entry>>,
    random: RandomState,
}

impl std::fmt::Debug for MappingGrowth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MappingGrowth").finish_non_exhaustive()
    }
}

/// A lock held across nothing that can panic half way: what it guards stays
/// consistent, so a poisoned one is recovered.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl MappingGrowth {
    /// Growth over `descriptors`, waiting between attempts by sleeping.
    pub fn new(descriptors: Arc<DescriptorStore>) -> Self {
        Self::with_sleeper(descriptors, Arc::new(std::thread::sleep))
    }

    /// Growth that waits between attempts with `sleep`.
    pub fn with_sleeper(descriptors: Arc<DescriptorStore>, sleep: Sleeper) -> Self {
        Self {
            descriptors,
            sleep,
            // ALLOC: one map, once a growth service.
            groups: Mutex::new(HashMap::new()),
            random: RandomState::new(),
        }
    }

    /// Adds `addition` to the mapping of `index` (whose uuid is `uuid`) in
    /// its descriptor, and answers the mapping the descriptor then holds.
    ///
    /// Callers for one index coalesce: the first to take the index's turn
    /// swaps once for every addition queued. A batch the store failed goes
    /// back on the queue, so the next caller carries it.
    ///
    /// # Errors
    ///
    /// [`GrowthError::Conflict`] when the mapping refuses the addition,
    /// [`GrowthError::Deleted`] for an index that is gone or recreated,
    /// [`GrowthError::Concurrent`] after [`ATTEMPTS`] lost swaps, and
    /// [`GrowthError::Store`] when the descriptor cannot be read or written.
    pub fn grow(
        &self,
        index: &str,
        uuid: &str,
        addition: &Mapping,
    ) -> Result<Mapping, GrowthError> {
        // ALLOC: one key a growth.
        let key = format!("{index}/{uuid}");
        let group = self.enter(&key);
        let out = self.take_turns(&group, index, uuid, addition);
        self.leave(&key);
        out
    }

    fn enter(&self, key: &str) -> Arc<Group> {
        let mut groups = lock(&self.groups);
        // ALLOC: one key a growth, for the group in flight.
        let entry = groups.entry(key.to_owned()).or_insert_with(|| Entry {
            group: Arc::new(Group::default()),
            users: 0,
        });
        entry.users = entry.users.saturating_add(1);
        Arc::clone(&entry.group)
    }

    /// Drops the group once no caller can still queue or wait on it.
    fn leave(&self, key: &str) {
        let mut groups = lock(&self.groups);
        if let Some(entry) = groups.get_mut(key) {
            entry.users = entry.users.saturating_sub(1);
            if entry.users == 0 {
                groups.remove(key);
            }
        }
    }

    fn take_turns(&self, group: &Group, index: &str, uuid: &str, addition: &Mapping) -> Answer {
        let answer: Arc<Mutex<Option<Answer>>> = Arc::new(Mutex::new(None));
        // ALLOC: one queued addition a growth, bounded by the field limit's
        // worth of distinct fields.
        lock(&group.pending).push(Waiter {
            addition: addition.clone(),
            answer: Arc::clone(&answer),
        });
        let _turn = lock(&group.turn);
        if let Some(done) = lock(&answer).take() {
            // A leader grew the mapping for this caller while it waited.
            return done;
        }
        let batch: Vec<Waiter> = std::mem::take(&mut *lock(&group.pending));
        // ALLOC: the batch's additions, at most the callers queued on one index.
        let additions: Vec<Mapping> = batch.iter().map(|w| w.addition.clone()).collect();
        let wait = |bound: u64| {
            (self.sleep)(attempts::jittered(bound, self.random.hash_one(bound)));
        };
        let result = attempts::grow_by(&self.descriptors, index, uuid, &additions, &wait);
        match result {
            Err(GrowthError::Store(m)) => {
                // Waiting callers retry the whole uncommitted batch.
                lock(&group.pending).extend(batch);
                Err(GrowthError::Store(m))
            }
            Err(GrowthError::Conflict(_)) if additions.len() > 1 => {
                // One addition is incompatible: answer each on its own, so
                // the compatible ones still grow the mapping.
                for waiter in batch {
                    let one = std::slice::from_ref(&waiter.addition);
                    let own = attempts::grow_by(&self.descriptors, index, uuid, one, &wait);
                    *lock(&waiter.answer) = Some(own);
                }
                lock(&answer).take().unwrap_or(Err(GrowthError::Store(
                    // ALLOC: error path; one message.
                    "a growth was not answered".to_owned(),
                )))
            }
            done => {
                for waiter in batch {
                    *lock(&waiter.answer) = Some(done.clone());
                }
                lock(&answer).take().unwrap_or(done)
            }
        }
    }
}

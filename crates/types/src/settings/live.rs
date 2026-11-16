//! The cluster register's settings as a running node holds them, swapped
//! whole when a newer read arrives.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

use super::Source;

/// One read of the cluster register's settings: its generation and its
/// values, flat.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// The register generation it was read at; 0 for none read yet.
    pub generation: u64,
    /// The settings, by dotted key.
    pub values: BTreeMap<String, String>,
}

impl Source for Snapshot {
    fn value(&self, key: &str) -> Option<&str> {
        self.values.value(key)
    }
}

/// The node's current cluster settings. Readers take the snapshot and read
/// it with no lock held; a writer swaps in a newer one.
#[derive(Debug, Default)]
pub struct Live {
    current: RwLock<Arc<Snapshot>>,
}

impl Live {
    /// Settings with nothing read yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The snapshot in force: unchanged by any later swap.
    pub fn current(&self) -> Arc<Snapshot> {
        // A poisoned lock still holds a whole snapshot: a swap is one store.
        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Puts `snapshot` in force if its generation is newer than the one
    /// held: a read that lost a race to a
    /// newer one changes nothing. Whether it was applied.
    pub fn replace(&self, snapshot: Snapshot) -> bool {
        let mut current = self.current.write().unwrap_or_else(PoisonError::into_inner);
        if snapshot.generation <= current.generation {
            return false;
        }
        *current = Arc::new(snapshot);
        true
    }
}

//! One parsed mapping for every shard opened from the same descriptor text.
//!
//! A parsed mapping keeps about 1.4 KB a field. A node that holds thousands
//! of open shards of a few mappings parses each distinct text once and hands
//! every shard the same [`Arc`]. The table keeps only weak references, so a
//! mapping no shard holds is freed, and its key is dropped by the lookup that
//! finds it dead or by a sweep of its lock shard.
//!
//! The key is the exact text, compared in full on a hit, so a hash collision
//! cannot hand a shard another mapping. The parse options of
//! [`Mapping::from_descriptor`] are fixed, so they are not part of the key; a
//! parse option that varies with an index would have to be.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use super::{Mapping, MappingError};

/// Lock shards; a hit contends only with texts of the same shard.
const LOCK_SHARDS: usize = 16;

/// The fewest entries a lock shard holds before it is swept.
const SWEEP_FLOOR: usize = 64;

#[derive(Default)]
struct Table {
    entries: HashMap<Arc<str>, Weak<Mapping>>,
    /// Sweep dead entries once the table holds this many.
    sweep_at: usize,
}

/// Parsed mappings by descriptor text, shared across shards.
pub struct MappingInterner {
    tables: Vec<Mutex<Table>>,
}

impl Default for MappingInterner {
    fn default() -> Self {
        Self::new()
    }
}

impl MappingInterner {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        // ALLOC: the lock shards, once a node.
        let tables = (0..LOCK_SHARDS)
            .map(|_| {
                Mutex::new(Table {
                    entries: HashMap::new(),
                    sweep_at: SWEEP_FLOOR,
                })
            })
            .collect();
        Self { tables }
    }

    fn table(&self, text: &str) -> &Mutex<Table> {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let at = usize::try_from(hasher.finish()).unwrap_or(0);
        // The remainder by a non-zero count is below it.
        let at = at.checked_rem(LOCK_SHARDS).unwrap_or(0);
        self.tables.get(at).unwrap_or(&self.tables[0])
    }

    /// The parsed mapping of descriptor text `text`, shared with every other
    /// holder of the same text; a missing mapping is the empty one.
    ///
    /// # Errors
    ///
    /// As [`Mapping::from_descriptor`]; a text that does not parse is not
    /// stored.
    pub fn get(&self, text: Option<&str>) -> Result<Arc<Mapping>, MappingError> {
        let Some(text) = text else {
            // ALLOC: the empty mapping, a few words.
            return Ok(Arc::new(Mapping::default()));
        };
        let table = self.table(text);
        {
            let mut held = table.lock().unwrap_or_else(PoisonError::into_inner);
            match held.entries.get(text).map(Weak::upgrade) {
                Some(Some(shared)) => return Ok(shared),
                Some(None) => {
                    held.entries.remove(text);
                }
                None => {}
            }
        }
        // A miss parses outside the lock: a slow parse blocks no hit.
        // ALLOC: one parsed mapping, bounded by the mapping text limit.
        let parsed = Arc::new(Mapping::from_descriptor(Some(text))?);
        let mut held = table.lock().unwrap_or_else(PoisonError::into_inner);
        // A racing miss that finished first wins, so the text keeps one copy.
        if let Some(Some(shared)) = held.entries.get(text).map(Weak::upgrade) {
            return Ok(shared);
        }
        if held.entries.len() >= held.sweep_at {
            held.entries.retain(|_, weak| weak.strong_count() > 0);
            held.sweep_at = held.entries.len().saturating_mul(2).max(SWEEP_FLOOR);
        }
        // ALLOC: one key a distinct live mapping, the length of its text.
        held.entries
            .insert(Arc::from(text), Arc::downgrade(&parsed));
        Ok(parsed)
    }

    /// How many entries the table holds, dead ones included.
    #[must_use]
    pub fn entries(&self) -> usize {
        self.tables
            .iter()
            .map(|t| {
                t.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .entries
                    .len()
            })
            .sum()
    }

    /// How many entries are held by a shard.
    #[must_use]
    pub fn live(&self) -> usize {
        self.tables
            .iter()
            .map(|t| {
                t.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .entries
                    .values()
                    .filter(|w| w.strong_count() > 0)
                    .count()
            })
            .sum()
    }
}

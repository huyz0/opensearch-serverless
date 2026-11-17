//! The versions of the ids written since the last refresh, as
//! `LiveVersionMap` keeps them, and the deletes kept past a refresh until
//! `index.gc_deletes` has passed.

use std::collections::HashMap;

use super::plan::Current;

/// `index.gc_deletes`' default: how long a delete's version outlives a
/// refresh, so a write soon after it continues the version.
pub const GC_DELETES_MILLIS: u64 = 60_000;

#[derive(Clone, Copy, Debug)]
struct Entry {
    current: Current,
    /// When a delete was written, for pruning it.
    at_millis: u64,
}

/// `LiveVersionMap`: the current version of each id written since the
/// last refresh, and of each id deleted within `gc_deletes`.
#[derive(Debug, Default)]
pub struct VersionMap {
    entries: HashMap<String, Entry>,
}

impl VersionMap {
    /// The current version of `id`, if a write since the last refresh, or
    /// a delete not yet pruned, set it.
    pub fn get(&self, id: &str) -> Option<Current> {
        self.entries.get(id).map(|e| e.current)
    }

    /// Records the write of `id` at `now_millis`.
    pub fn put(&mut self, id: &str, current: Current, now_millis: u64) {
        let entry = Entry {
            current,
            at_millis: now_millis,
        };
        match self.entries.get_mut(id) {
            Some(e) => *e = entry,
            None => {
                // ALLOC: one entry an id written since the last refresh, or deleted within gc_deletes.
                self.entries.insert(id.to_owned(), entry);
            }
        }
    }

    /// After a refresh: the segments hold every live document now, so only
    /// the deletes stay (`pruneTombstones` keeps them for `gc_deletes`).
    pub fn refreshed(&mut self) {
        self.entries.retain(|_, e| e.current.deleted);
    }

    /// Drops the deletes written `gc_deletes_millis` or more before
    /// `now_millis`.
    pub fn prune_deletes(&mut self, now_millis: u64, gc_deletes_millis: u64) {
        self.entries.retain(|_, e| {
            !e.current.deleted || now_millis.saturating_sub(e.at_millis) < gc_deletes_millis
        });
    }

    /// How many ids it holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(version: i64, deleted: bool) -> Current {
        Current {
            version,
            seq_no: version,
            term: 1,
            deleted,
        }
    }

    #[test]
    fn a_refresh_keeps_deletes_until_gc_deletes_has_passed() {
        let mut m = VersionMap::default();
        m.put("a", at(1, false), 0);
        m.put("a", at(2, false), 5);
        m.put("d", at(3, true), 10);
        assert_eq!(m.get("a"), Some(at(2, false)));
        assert_eq!(m.len(), 2);
        m.refreshed();
        assert_eq!((m.get("a"), m.get("d")), (None, Some(at(3, true))));
        m.prune_deletes(10 + GC_DELETES_MILLIS - 1, GC_DELETES_MILLIS);
        assert!(m.get("d").is_some());
        m.prune_deletes(10 + GC_DELETES_MILLIS, GC_DELETES_MILLIS);
        assert!(m.is_empty());
    }
}

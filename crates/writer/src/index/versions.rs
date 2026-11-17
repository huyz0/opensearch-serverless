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
    /// What the entries hold, as `LiveVersionMap.ramBytesUsed` counts it.
    bytes: usize,
}

/// The heap one entry holds: its key and value, and the map's slot.
fn entry_bytes(id: &str) -> usize {
    id.len()
        .saturating_add(std::mem::size_of::<String>())
        .saturating_add(std::mem::size_of::<Entry>())
        .saturating_add(std::mem::size_of::<u64>())
}

impl VersionMap {
    /// The current version of `id` at `now_millis`, if a write since the
    /// last refresh set it, or a delete less than `gc_deletes_millis` old
    /// (`resolveDocVersion` drops an older one at lookup).
    pub fn get(&self, id: &str, now_millis: u64, gc_deletes_millis: u64) -> Option<Current> {
        self.entries
            .get(id)
            .filter(|e| !e.current.deleted || !expired(e, now_millis, gc_deletes_millis))
            .map(|e| e.current)
    }

    /// The heap the entries hold, which counts toward the indexing buffer.
    pub fn ram_bytes(&self) -> usize {
        self.bytes
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
                self.bytes = self.bytes.saturating_add(entry_bytes(id));
                // ALLOC: one entry an id written since the last refresh, or deleted within gc_deletes; counted in ram_bytes, which the indexing buffer bounds.
                self.entries.insert(id.to_owned(), entry);
            }
        }
    }

    /// After a refresh: the segments hold every live document now, so only
    /// the deletes stay (`pruneTombstones` keeps them for `gc_deletes`).
    pub fn refreshed(&mut self) {
        self.entries.retain(|_, e| e.current.deleted);
        self.recount();
    }

    fn recount(&mut self) {
        self.bytes = self
            .entries
            .keys()
            .fold(0, |sum, id| sum.saturating_add(entry_bytes(id)));
    }

    /// Drops the deletes written `gc_deletes_millis` or more before
    /// `now_millis`.
    pub fn prune_deletes(&mut self, now_millis: u64, gc_deletes_millis: u64) {
        self.entries
            .retain(|_, e| !e.current.deleted || !expired(e, now_millis, gc_deletes_millis));
        self.recount();
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

/// `canRemoveTombstone`: a delete written more than `gc_deletes` ago.
fn expired(e: &Entry, now_millis: u64, gc_deletes_millis: u64) -> bool {
    now_millis.saturating_sub(e.at_millis) > gc_deletes_millis
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
        let gc = GC_DELETES_MILLIS;
        assert_eq!(m.get("a", 5, gc), Some(at(2, false)));
        assert_eq!(m.len(), 2);
        let two = m.ram_bytes();
        assert_eq!(two, 2 * entry_bytes("a"));
        m.refreshed();
        assert_eq!(m.ram_bytes(), entry_bytes("d"));
        assert_eq!(
            (m.get("a", 10, gc), m.get("d", 10, gc)),
            (None, Some(at(3, true)))
        );
        // Exactly gc_deletes old still counts; older does not, even unpruned.
        assert!(m.get("d", 10 + gc, gc).is_some());
        assert!(m.get("d", 11 + gc, gc).is_none());
        m.prune_deletes(10 + gc, gc);
        assert_eq!(m.len(), 1);
        m.prune_deletes(11 + gc, gc);
        assert!(m.is_empty() && m.ram_bytes() == 0);
    }
}

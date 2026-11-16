//! Listing names, a bounded page at a time.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/DescriptorListing.java`.
//! "List every index" is not an operation at target scale: the answer does
//! not fit a response and is stale before it ends. So a listing is a cursor
//! walk whose LISTs and reads are bounded by the page, and a prefix lookup
//! whose cost is set by its cap; neither scales with the deployment.
//!
//! Neither is a snapshot. A name created or deleted behind the cursor is
//! missed, one ahead of it seen or not; every index that exists for the
//! whole walk appears exactly once.

use oss_blob::{BlobError, MAX_LIST_KEYS};
use oss_format::{
    DESCRIPTORS_PREFIX, IndexRecord, MAX_TOMBSTONE_BYTES, NameRecord, TOMBSTONE_PREFIX,
};

use crate::descriptors::DescriptorStore;
use crate::error::MetaError;
use crate::names::validate_prefix;

impl DescriptorStore {
    /// One page of the indexes whose names start with `prefix` and sort
    /// after `after`, in name order: at most `limit`, each handed to `visit`
    /// borrowing `buf`. Answers the name to resume after, or `None` when the
    /// walk is complete.
    ///
    /// Tombstones and aliases take no row, so a page can be short, and the
    /// last one empty. Each LIST asks for [`MAX_LIST_KEYS`] keys and the page
    /// reads them in order until it is full, resuming after the last one
    /// read: tombstones cost reads, never a LIST each, so the LISTs are
    /// `ceil(keys read / MAX_LIST_KEYS)`.
    ///
    /// # Errors
    ///
    /// [`MetaError::InvalidName`] for a prefix no name can start with and
    /// [`MetaError::InvalidArgument`] for a `limit` of 0, before any request;
    /// [`MetaError::Format`] for a malformed index record or alias (unreadable
    /// is not absent); [`MetaError::Blob`] for a register longer than `buf`.
    pub fn list_page(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
        buf: &mut [u8],
        visit: &mut dyn FnMut(&IndexRecord<'_>),
    ) -> Result<Option<String>, MetaError> {
        validate_prefix(prefix)?;
        if limit == 0 {
            return Err(MetaError::InvalidArgument(
                "a page must take at least one name",
            ));
        }
        let names = self.names_prefix();
        // ALLOC: one listing prefix and one cursor a page, keys long.
        let listing = [names.as_str(), prefix].concat();
        let mut cursor = after.map(|a| [names.as_str(), a].concat());
        // ALLOC: one LIST's keys, at most MAX_LIST_KEYS of MAX_KEY_BYTES.
        let mut keys = Vec::with_capacity(MAX_LIST_KEYS);
        let mut taken = 0usize;
        loop {
            // LIST: one page of keys, read until the page is full; bounded
            // by the keys the page reads, never by the population.
            let truncated =
                self.list_keys(&listing, cursor.as_deref(), MAX_LIST_KEYS, &mut keys)?;
            let mut read_to = 0usize;
            for key in &keys {
                if taken >= limit {
                    break;
                }
                read_to = read_to.saturating_add(1);
                if nested(key, names.len()) {
                    continue;
                }
                let Some(read) = self.store.read_register(key, buf)? else {
                    // Deleted since it was listed.
                    continue;
                };
                let value = buf.get(..read.len).unwrap_or_default();
                if value.starts_with(TOMBSTONE_PREFIX) {
                    // Deleted: recognised, not parsed, as the Java's.
                    continue;
                }
                // An alias is recognised, not decoded, as the Java's.
                if let Some(record) = NameRecord::index_only(value)? {
                    visit(&record);
                    taken = taken.saturating_add(1);
                }
            }
            let left = read_to < keys.len();
            keys.truncate(read_to);
            let Some(last) = keys.pop() else {
                return Ok(None);
            };
            if !left && !truncated {
                return Ok(None);
            }
            if taken >= limit {
                // A cursor, not a total: counting them all is a full scan.
                // ALLOC: the name to resume after.
                return Ok(Some(last.get(names.len()..).unwrap_or_default().to_owned()));
            }
            // Tombstones left the page short: list on from the last name.
            cursor = Some(last);
        }
    }

    /// The names that start with `prefix`, in order, at most `cap`: what an
    /// index pattern such as `logs-*` matches, at a cost set by the cap.
    ///
    /// When at most `cap` keys match they are answered unread, tombstones
    /// included: the caller reads each to learn what it holds. Only when
    /// more match are they read, so that a prefix crowded with deleted
    /// names is not refused; the live ones (an index or an alias) are then
    /// answered and the tombstones dropped.
    ///
    /// # Errors
    ///
    /// [`MetaError::TooManyMatches`] past `cap` live names;
    /// [`MetaError::InvalidName`] and [`MetaError::InvalidArgument`] as
    /// [`DescriptorStore::list_page`]'s, before any request.
    pub fn names_with_prefix(&self, prefix: &str, cap: usize) -> Result<Vec<String>, MetaError> {
        validate_prefix(prefix)?;
        if cap == 0 {
            return Err(MetaError::InvalidArgument(
                "a cap must allow at least one name",
            ));
        }
        let names = self.names_prefix();
        // ALLOC: one listing prefix a call.
        let listing = [names.as_str(), prefix].concat();
        // ALLOC: up to cap names, the bound the caller chose.
        let mut found: Vec<String> = Vec::new();
        // ALLOC: one LIST's keys, at most MAX_LIST_KEYS of MAX_KEY_BYTES.
        let mut keys = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let want = cap.saturating_sub(found.len()).min(MAX_LIST_KEYS);
            // LIST: up to the cap, so the cost is the cap's, not the population's.
            let truncated = self.list_keys(&listing, cursor.as_deref(), want, &mut keys)?;
            for key in &keys {
                if !nested(key, names.len()) {
                    // ALLOC: one name, bounded by the cap as above.
                    found.push(key.get(names.len()..).unwrap_or_default().to_owned());
                }
            }
            let Some(last) = keys.pop() else {
                return Ok(found);
            };
            if !truncated {
                return Ok(found);
            }
            cursor = Some(last);
            if found.len() >= cap {
                break;
            }
        }
        // More keys than the cap, deleted names included: keep the live
        // ones read so far, then read on until the walk ends or they pass
        // the cap. Reads are bounded by the cap plus the tombstones among
        // them, which the sweep bounds.
        let mut kept = 0;
        for at in 0..found.len() {
            let live = found
                .get(at)
                .map(|name| [names.as_str(), name].concat())
                .map_or(Ok(false), |key| self.is_live(&key))?;
            if live {
                found.swap(kept, at);
                kept = kept.saturating_add(1);
            }
        }
        found.truncate(kept);
        loop {
            // LIST: the rest of a prefix past its cap, stopped at cap + 1 live names.
            let truncated =
                self.list_keys(&listing, cursor.as_deref(), MAX_LIST_KEYS, &mut keys)?;
            for key in &keys {
                if nested(key, names.len()) || !self.is_live(key)? {
                    continue;
                }
                if found.len() >= cap {
                    return Err(MetaError::TooManyMatches {
                        // ALLOC: one copy of the prefix, for the message.
                        prefix: prefix.to_owned(),
                        cap,
                    });
                }
                // ALLOC: one name, at most cap of them as above.
                found.push(key.get(names.len()..).unwrap_or_default().to_owned());
            }
            let Some(last) = keys.pop() else {
                return Ok(found);
            };
            if !truncated {
                return Ok(found);
            }
            cursor = Some(last);
        }
    }

    /// `<base>indices/`.
    // ALLOC: one prefix a call.
    fn names_prefix(&self) -> String {
        [self.base.as_str(), DESCRIPTORS_PREFIX].concat()
    }

    /// One LIST into `keys`, cleared first; whether more keys follow.
    fn list_keys(
        &self,
        listing: &str,
        cursor: Option<&str>,
        want: usize,
        keys: &mut Vec<String>,
    ) -> Result<bool, MetaError> {
        keys.clear();
        let page = self
            .store
            // LIST: one page of at most `want` keys; each caller bounds `want`.
            .list_prefix(listing, cursor, want, &mut |key, _| {
                // ALLOC: one key, at most `want` of them.
                keys.push(key.to_owned());
            })?;
        Ok(page.truncated)
    }

    /// Whether the register at `key` holds a live name: an index, an
    /// alias, or anything else that is not a tombstone. Read into a
    /// tombstone's length, so a longer register is live without being
    /// fetched whole.
    fn is_live(&self, key: &str) -> Result<bool, MetaError> {
        let mut buf = [0u8; MAX_TOMBSTONE_BYTES];
        match self.store.read_register(key, &mut buf) {
            Ok(Some(read)) => Ok(!buf
                .get(..read.len)
                .unwrap_or_default()
                .starts_with(TOMBSTONE_PREFIX)),
            Ok(None) => Ok(false),
            // The key was just listed, so the only argument refused is the
            // buffer: too short for anything but a tombstone.
            Err(BlobError::InvalidArgument(_)) => Ok(true),
            Err(e) => Err(e.into()),
        }
    }
}

/// Whether a listed key is below a name rather than a name: names hold no
/// `/`, so such a key is not a register this store wrote.
fn nested(key: &str, names_len: usize) -> bool {
    key.get(names_len..).is_none_or(|name| name.contains('/'))
}

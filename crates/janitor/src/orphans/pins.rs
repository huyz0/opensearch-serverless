//! What keeps a dead index's shard containers: the points in time and the
//! snapshots that name its incarnation.
//!
//! A pin here is an index incarnation, `(name, uuid)`: every shard of it is
//! kept, whichever shards the record names, which holds at least what the
//! Java's `PointInTime.pins` and `SnapshotRecord.referencedBlobs` hold. A
//! source that cannot read a record fails the slice: unreadable is not
//! "none", and there is no source that answers "no pins".

use std::collections::HashSet;
use std::sync::Arc;

use oss_blob::{BlobError, BlobStore, ByteRange, MAX_LIST_KEYS};
use oss_format::{MAX_POINT_IN_TIME_BYTES, snapshot_pinned_indexes};
use oss_meta::PointInTimeStore;

use crate::JanitorError;

/// The index incarnations pinned, bounded.
#[derive(Debug)]
pub struct IndexPins {
    incarnations: HashSet<(String, String)>,
    // Names pinned whatever their uuid: a view that does not know one.
    names: HashSet<String>,
    max: usize,
}

impl IndexPins {
    /// An empty set that refuses more than `max` entries.
    pub fn new(max: usize) -> Self {
        Self {
            incarnations: HashSet::new(),
            names: HashSet::new(),
            max,
        }
    }

    /// Pins the incarnation `uuid` of `index`; `None` pins every incarnation
    /// of the name.
    ///
    /// # Errors
    ///
    /// [`JanitorError::Pins`] past the cap: nothing is deleted on a set that
    /// cannot hold what is pinned.
    pub fn add(&mut self, index: &str, uuid: Option<&str>) -> Result<(), JanitorError> {
        match uuid {
            // ALLOC: one entry a pinned incarnation, at most `max`.
            Some(uuid) => {
                self.incarnations
                    .insert((index.to_owned(), uuid.to_owned()));
            }
            // ALLOC: one name a pinned name, at most `max`.
            None => {
                self.names.insert(index.to_owned());
            }
        }
        if self.incarnations.len().saturating_add(self.names.len()) > self.max {
            return Err(JanitorError::Pins("more pins than the cap"));
        }
        Ok(())
    }

    /// Whether the incarnation is pinned.
    pub fn contains(&self, index: &str, uuid: &str) -> bool {
        self.names.contains(index)
            // ALLOC: one probe key.
            || self
                .incarnations
                .contains(&(index.to_owned(), uuid.to_owned()))
    }
}

/// One kind of record that pins index incarnations.
pub trait PinSource: Send + Sync {
    /// Adds every pin the source holds to `into`, reading all its records.
    ///
    /// # Errors
    ///
    /// [`JanitorError`] when a record cannot be read or there are more pins
    /// or records than the caps allow; nothing is deleted.
    fn collect(&self, into: &mut IndexPins) -> Result<(), JanitorError>;
}

/// The live points in time.
pub struct PointInTimeSource<'a> {
    store: &'a PointInTimeStore,
    max_records: usize,
}

impl<'a> PointInTimeSource<'a> {
    /// The views of `store`, at most `max_records` of them live.
    pub fn new(store: &'a PointInTimeStore, max_records: usize) -> Self {
        Self { store, max_records }
    }
}

impl PinSource for PointInTimeSource<'_> {
    /// Cost: one LIST page per 1,000 views and a register read each.
    fn collect(&self, into: &mut IndexPins) -> Result<(), JanitorError> {
        // ALLOC: one record buffer, the largest a view record may be.
        let mut buf = vec![0u8; MAX_POINT_IN_TIME_BYTES.min(1 << 20)];
        let views = self.store.live(&mut buf)?;
        if views.len() > self.max_records {
            return Err(JanitorError::Pins("more points in time than the cap"));
        }
        for view in &views {
            if view.is_placeholder() {
                return Err(JanitorError::Pins("a point in time could not be read"));
            }
            // A capture holds the whole index whatever incarnation it names,
            // as the Java's `pins(index, null)`.
            let uuid = if view.capturing {
                None
            } else {
                view.index_uuid.as_deref()
            };
            into.add(&view.index, uuid)?;
        }
        Ok(())
    }
}

/// The snapshot records of the deployment, read from the inventory under
/// `<base>snapshots/`: each snapshot's marker holds the index incarnations
/// it captured, finished or not.
pub struct SnapshotSource {
    store: Arc<dyn BlobStore>,
    prefix: String,
    max_records: usize,
    max_record_bytes: usize,
}

impl SnapshotSource {
    /// The snapshots under `base` (empty, or ending in `/`), at most
    /// `max_records` of them, each at most `max_record_bytes` long.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        max_records: usize,
        max_record_bytes: usize,
    ) -> Self {
        Self {
            store,
            // ALLOC: the prefix, once a source.
            prefix: format!("{base}snapshots/"),
            max_records,
            max_record_bytes,
        }
    }
}

impl PinSource for SnapshotSource {
    /// Cost: one LIST page per 1,000 snapshots and a GET each.
    fn collect(&self, into: &mut IndexPins) -> Result<(), JanitorError> {
        // ALLOC: one record buffer, at most `max_record_bytes`.
        let mut buf = vec![0u8; self.max_record_bytes];
        self.collect_with(into, &mut buf)
    }
}

impl SnapshotSource {
    /// As [`PinSource::collect`], reading each record through `buf` (at most
    /// `max_record_bytes` of a record are kept whatever its size).
    ///
    /// # Errors
    ///
    /// As [`PinSource::collect`].
    pub fn collect_with(&self, into: &mut IndexPins, buf: &mut [u8]) -> Result<(), JanitorError> {
        // ALLOC: one key a snapshot, at most `max_records` (checked below).
        let mut keys: Vec<String> = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let mut last = None;
            // LIST: one page per 1,000 snapshots, the slice's pin read.
            let page = self.store.list_prefix(
                &self.prefix,
                after.as_deref(),
                MAX_LIST_KEYS,
                &mut |key, _| {
                    // ALLOC: one key, as above.
                    keys.push(key.to_owned());
                    last = Some(key.to_owned());
                },
            )?;
            if keys.len() > self.max_records {
                return Err(JanitorError::Pins("more snapshots than the cap"));
            }
            if !page.truncated {
                break;
            }
            after = last;
        }
        for key in &keys {
            let mut body: Vec<u8> = Vec::new();
            let got = self
                .store
                .get(key, ByteRange::Full, buf, &mut |chunk: &[u8]| -> Result<
                    (),
                    BlobError,
                > {
                    // ALLOC: the record, at most `max_record_bytes` (checked).
                    body.extend_from_slice(chunk);
                    if body.len() > self.max_record_bytes {
                        return Err(BlobError::InvalidArgument("a snapshot record too long"));
                    }
                    Ok(())
                });
            match got {
                Ok(_) => {}
                // Deleted since the listing: it pins nothing.
                Err(BlobError::NotFound) => continue,
                Err(e) => return Err(e.into()),
            }
            let indexes = snapshot_pinned_indexes(&body)
                .map_err(|_| JanitorError::Pins("a snapshot record could not be read"))?;
            for (name, uuid) in &indexes {
                into.add(name, Some(uuid))?;
            }
        }
        Ok(())
    }
}

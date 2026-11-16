//! Index descriptors on registers: one register a name, changed only by
//! compare-and-swap.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/DescriptorStore.java`.
//! The register `indices/<name>` holds an index's descriptor, an alias, or a
//! deleted index's tombstone. The store arbitrates every race: of two
//! creates of one name one applies, of two updates from one generation one
//! applies, and a delete applies only to the generation it read.
//!
//! A deleted name keeps its register as a tombstone, unless the store
//! honours a conditional delete: a register created again then starts at a
//! fresh random generation. Either way a swap carrying a generation read
//! before the delete cannot land on the index created after it.
//!
//! Every call takes the caller's buffer, which bounds the record it reads or
//! writes; nothing here allocates on the request path.

use std::sync::Arc;

use oss_blob::{BlobError, BlobStore, SliceSource, SwapOutcome};
use oss_format::ChangeKind;
use oss_format::{
    IndexRecord, MAX_TOMBSTONE_BYTES, NameRecord, TOMBSTONE_PREFIX, Tombstone, descriptor_key,
    tombstone_bucket, tombstone_marker_key,
};

use crate::changes::ChangeStripes;
use crate::error::MetaError;
use crate::names::validate_index_or_alias;

/// How long after a sweep claims a tombstone its name stays refused
/// (`TOMBSTONE_REAP_HANDOFF_MILLIS`): the sweep may still be removing the old
/// index's data.
pub const REAP_HANDOFF_MILLIS: i64 = 60_000;

/// How long a sweep's claim licenses its delete
/// (`TOMBSTONE_REAP_DEADLINE_MILLIS`): well inside the handoff, so a create
/// cannot have taken the name over in between.
pub const REAP_DEADLINE_MILLIS: i64 = 10_000;

/// How long a tombstone stands before a sweep may remove it
/// (`DEFAULT_TOMBSTONE_QUARANTINE_MILLIS`): no operation carrying a generation
/// from before the delete is in flight that long.
pub const DEFAULT_QUARANTINE_MILLIS: i64 = 6 * 60 * 60 * 1000;

/// A marker bucket's span (`TOMBSTONE_BUCKET_MILLIS`): one UTC hour.
pub const BUCKET_MILLIS: i64 = 60 * 60 * 1000;

/// How a delete keeps a recreated name from meeting an old generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode {
    /// The store refuses a delete whose register moved (S3's `If-Match` on
    /// DELETE), so the register is removed outright. Only for a store known
    /// to honour it: one that ignores the condition deletes regardless.
    Conditional,
    /// The register is swapped to a tombstone, after a marker under its
    /// hour's bucket for the sweep to find it by.
    Tombstone,
}

/// What a name resolves to, with the register's generation
/// ([`oss_blob::register::ABSENT_GENERATION`] for none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution<'b> {
    /// The register's generation, to swap from.
    pub generation: u64,
    /// What the register holds.
    pub target: Target<'b>,
}

/// What a name's register holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target<'b> {
    /// An index.
    Index(IndexRecord<'b>),
    /// An alias, as its record's bytes. One still being created is returned
    /// as it is: telling needs the alias record decoded.
    Alias(&'b [u8]),
    /// No register, or a tombstone.
    Absent,
}

/// Index descriptors in one deployment's registers.
pub struct DescriptorStore {
    pub(crate) store: Arc<dyn BlobStore>,
    pub(crate) base: String,
    pub(crate) deletes: DeleteMode,
    stripes: Option<ChangeStripes>,
}

impl DescriptorStore {
    /// The descriptors under `base` (empty, or ending in `/`) of `store`,
    /// deleted as `deletes` says.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a base that is not empty and does not end in
    /// `/`.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        deletes: DeleteMode,
    ) -> Result<Self, MetaError> {
        descriptor_key(base, "probe")?;
        Ok(Self {
            store,
            // ALLOC: once a store, the deployment's base.
            base: base.to_owned(),
            deletes,
            stripes: None,
        })
    }

    /// Records every update and delete of a descriptor in its change
    /// stripe, as the Java's `recordChangesIn`. Every node of a deployment
    /// must record them for any writer to confirm an index by its stripe.
    /// An update then costs a stripe read and swap more, a delete two.
    #[must_use]
    pub fn record_changes_in(mut self, stripes: ChangeStripes) -> Self {
        self.stripes = Some(stripes);
        self
    }

    /// Records an applied change. A failure leaves writers' mappings a
    /// little staler, and is not the caller's: the change applied.
    fn changed(&self, name: &str, kind: ChangeKind) {
        if let Some(stripes) = &self.stripes {
            // After a delete its pending entry stays, and the stripe never
            // confirms the name again: safe, and slower.
            let _ = stripes.bump(name, kind);
        }
    }

    /// Records a delete about to be applied. A failure refuses the delete:
    /// a writer could otherwise go on confirming the index by its stripe.
    /// A name a stripe cannot hold (longer than any name the rules allow,
    /// or holding a newline) is not recorded: no writer confirms it by its
    /// stripe, and its delete stays idempotent.
    fn about_to_delete(&self, name: &str) -> Result<(), MetaError> {
        match &self.stripes {
            Some(stripes) if ChangeStripes::holds(name) => {
                stripes.bump(name, ChangeKind::PendingDelete)
            }
            _ => Ok(()),
        }
    }

    /// The name's register key, once the name is checked.
    // ALLOC: one key a call, at most the base and a 255-byte name.
    fn key(&self, name: &str) -> Result<String, MetaError> {
        validate_index_or_alias(name)?;
        self.existing_key(name)
    }

    /// The register key of a name already in the store, unchecked against
    /// the name rules, as the Java's update and delete: a name a later rule
    /// refuses can still be changed and removed. It is still a single key
    /// segment (`descriptor_key` refuses `/` and dot segments).
    // ALLOC: one key a call, at most the base and the name.
    pub(crate) fn existing_key(&self, name: &str) -> Result<String, MetaError> {
        Ok(descriptor_key(&self.base, name)?)
    }

    /// Creates the index `record` names, returning its register's
    /// generation. A name never used costs one request; only a taken one is
    /// read, to tell a tombstone, which is swapped over, from a live record.
    ///
    /// # Errors
    ///
    /// [`MetaError::AlreadyExists`] when the name holds an index or alias, or
    /// a racing create or swap took it first; [`MetaError::BeingReclaimed`]
    /// when its tombstone was claimed under [`REAP_HANDOFF_MILLIS`] before
    /// `now_millis`; [`MetaError::Format`] when the record does not fit `buf`
    /// or the taken register is malformed.
    pub fn create(
        &self,
        record: &IndexRecord<'_>,
        now_millis: i64,
        buf: &mut [u8],
    ) -> Result<u64, MetaError> {
        let key = self.key(&record.name)?;
        let len = record.write(buf)?;
        if let SwapOutcome::Applied(generation) =
            self.store.create_register(&key, prefix(buf, len))?
        {
            return Ok(generation);
        }
        // A tombstone is short: read into a buffer of its size, so a live
        // record of any length is never fetched whole just to be refused,
        // and one longer than the buffer is, like any non-tombstone, taken.
        let mut taken = [0u8; MAX_TOMBSTONE_BYTES];
        let current = match self.store.read_register(&key, &mut taken) {
            Ok(Some(current)) => current,
            // Taken a moment ago and gone again: still not this create's.
            Ok(None) => return Err(MetaError::AlreadyExists),
            // The key is valid (the create took it), so the only argument
            // refused is the buffer: too long for a tombstone.
            Err(BlobError::InvalidArgument(_)) => return Err(MetaError::AlreadyExists),
            Err(e) => return Err(e.into()),
        };
        let value = prefix(&taken, current.len);
        if !value.starts_with(TOMBSTONE_PREFIX) {
            return Err(MetaError::AlreadyExists);
        }
        let reaping_at = Tombstone::parse(value)?.reaping_at;
        if reaping_at != 0 && now_millis < reaping_at.saturating_add(REAP_HANDOFF_MILLIS) {
            return Err(MetaError::BeingReclaimed);
        }
        match self
            .store
            .swap_register(&key, current.generation, prefix(buf, len))?
        {
            SwapOutcome::Applied(generation) => Ok(generation),
            SwapOutcome::Conflict(_) => Err(MetaError::AlreadyExists),
        }
    }

    /// The index named `name`, or `None` when there is none: no register, a
    /// tombstone, or an alias. The record borrows from `buf`.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a malformed register: unreadable is not
    /// absent.
    pub fn get<'b>(
        &self,
        name: &str,
        buf: &'b mut [u8],
    ) -> Result<Option<IndexRecord<'b>>, MetaError> {
        Ok(match self.resolve_raw(name, buf)?.target {
            Target::Index(record) => Some(record),
            Target::Alias(_) | Target::Absent => None,
        })
    }

    /// What `name` resolves to and the generation to swap from: one read.
    ///
    /// # Errors
    ///
    /// As [`DescriptorStore::get`].
    pub fn resolve_raw<'b>(
        &self,
        name: &str,
        buf: &'b mut [u8],
    ) -> Result<Resolution<'b>, MetaError> {
        let key = self.key(name)?;
        let Some(read) = self.store.read_register(&key, buf)? else {
            return Ok(Resolution {
                generation: oss_blob::register::ABSENT_GENERATION,
                target: Target::Absent,
            });
        };
        let target = match NameRecord::parse(prefix(buf, read.len))? {
            NameRecord::Index(record) => Target::Index(record),
            NameRecord::Alias(bytes) => Target::Alias(bytes),
            NameRecord::Tombstone(_) => Target::Absent,
        };
        Ok(Resolution {
            generation: read.generation,
            target,
        })
    }

    /// The generation of `name`'s register, or
    /// [`oss_blob::register::ABSENT_GENERATION`]: one read, through `buf`.
    ///
    /// # Errors
    ///
    /// [`MetaError::Blob`] when the register does not fit `buf`.
    pub fn generation_of(&self, name: &str, buf: &mut [u8]) -> Result<u64, MetaError> {
        let key = self.key(name)?;
        Ok(self
            .store
            .read_register(&key, buf)?
            .map_or(oss_blob::register::ABSENT_GENERATION, |r| r.generation))
    }

    /// Replaces the descriptor of `record`'s name if its register still
    /// holds `expected`; the new generation, or `None` when it moved.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] when the record does not fit `buf`.
    pub fn update(
        &self,
        record: &IndexRecord<'_>,
        expected: u64,
        buf: &mut [u8],
    ) -> Result<Option<u64>, MetaError> {
        let key = self.existing_key(&record.name)?;
        let len = record.write(buf)?;
        Ok(
            match self.store.swap_register(&key, expected, prefix(buf, len))? {
                SwapOutcome::Applied(generation) => {
                    self.changed(&record.name, ChangeKind::Changed);
                    Some(generation)
                }
                SwapOutcome::Conflict(_) => None,
            },
        )
    }

    /// Removes `name`'s register whatever it holds; removing none succeeds.
    ///
    /// # Errors
    ///
    /// [`MetaError::Blob`] when the store fails.
    pub fn delete(&self, name: &str) -> Result<(), MetaError> {
        let key = self.existing_key(name)?;
        self.about_to_delete(name)?;
        let deleted = self.store.delete(&key);
        // Applied or failed, the attempt is over.
        self.changed(name, ChangeKind::Deleted);
        Ok(deleted?)
    }

    /// Deletes the index named `name` if its register still holds
    /// `expected`: `false` when it moved. In [`DeleteMode::Tombstone`] the
    /// marker is written first, so a crash between the two leaves a marker
    /// the sweep skips rather than a tombstone it never finds.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a `now_millis` outside 1970 to 9999, which
    /// has no bucket.
    pub fn delete_if_unchanged(
        &self,
        name: &str,
        expected: u64,
        now_millis: i64,
    ) -> Result<bool, MetaError> {
        let key = self.existing_key(name)?;
        self.about_to_delete(name)?;
        let deleted = self.delete_recorded(&key, name, expected, now_millis);
        // Applied, refused or failed, the attempt is over; a crash before
        // this leaves the pending entry, which is safe.
        self.changed(name, ChangeKind::Deleted);
        deleted
    }

    fn delete_recorded(
        &self,
        key: &str,
        name: &str,
        expected: u64,
        now_millis: i64,
    ) -> Result<bool, MetaError> {
        if self.deletes == DeleteMode::Conditional {
            return Ok(self.store.delete_register(key, expected)?);
        }
        let bucket = tombstone_bucket(now_millis)?;
        let bucket = std::str::from_utf8(&bucket).unwrap_or_default();
        let marker = tombstone_marker_key(&self.base, bucket, name)?;
        self.store
            .put(&marker, &mut SliceSource::new(&[]), &mut [0u8; 16])?;
        let mut tombstone = [0u8; MAX_TOMBSTONE_BYTES];
        let len = Tombstone {
            deleted_at: now_millis,
            reaping_at: 0,
        }
        .write(&mut tombstone)?;
        Ok(matches!(
            self.store
                .swap_register(key, expected, prefix(&tombstone, len))?,
            SwapOutcome::Applied(_)
        ))
    }
}

/// The first `len` bytes of `buf`, which a write or read just filled.
fn prefix(buf: &[u8], len: usize) -> &[u8] {
    buf.get(..len).unwrap_or_default()
}

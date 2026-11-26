//! Restoring one index of a shallow snapshot: create its descriptor, copy
//! every captured file of every shard into the new index's first-term
//! container, and land each shard's manifest only if none is there, as the
//! shell's `SnapshotOperations.restore` and `SnapshotBlobs.copyShardBlobs`
//! do.
//!
//! The copy reads the source in ranges of one buffer and writes each file as
//! a blob of its own, so memory is two buffers whatever a file's length.
//! A restore makes no LIST: every file's length is in the record, and a
//! record that does not state one is refused before anything is written.

use std::borrow::Cow;
use std::sync::Arc;

use oss_blob::{BlobError, BlobStore, SwapOutcome};
use oss_format::{
    EMPTY_SETTINGS, FileLocation, FormatError, IndexRecord, ManifestEntry, RestoreIndex,
    RestoreShard, ShardRef, encode_manifest, mark_restoring,
};

use crate::descriptors::{DescriptorStore, Target};
use crate::error::MetaError;
use crate::ranged_source::RangedSource;

/// The term container a restored shard's files are written to: the new
/// index's first term.
pub(crate) const FIRST_TERM_DIR: &str = "t=1";

/// Why a restore of one index failed.
#[derive(Debug)]
pub enum RestoreError {
    /// The record holds something a restore does not carry; nothing was
    /// written by this index.
    Unsupported(&'static str),
    /// A writer published on the new shard before the restored manifest
    /// could land: the restored commit is not in place.
    Conflict {
        /// The shard.
        shard: u32,
    },
    /// The descriptor could not be created.
    Meta(MetaError),
    /// The store failed.
    Blob(BlobError),
    /// A record or key was malformed.
    Format(FormatError),
    /// The node is stopping and the restore was told to give up; the index
    /// it created is left marked for the caller to delete.
    Cancelled,
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(why) => write!(f, "{why}"),
            Self::Conflict { shard } => {
                write!(
                    f,
                    "shard {shard} was published on before the restore landed"
                )
            }
            Self::Meta(e) => write!(f, "{e}"),
            Self::Blob(e) => write!(f, "{e}"),
            Self::Format(e) => write!(f, "{e}"),
            Self::Cancelled => write!(f, "the restore was cancelled because the node is stopping"),
        }
    }
}

impl std::error::Error for RestoreError {}

impl From<MetaError> for RestoreError {
    fn from(e: MetaError) -> Self {
        Self::Meta(e)
    }
}

impl From<BlobError> for RestoreError {
    fn from(e: BlobError) -> Self {
        Self::Blob(e)
    }
}

impl From<FormatError> for RestoreError {
    fn from(e: FormatError) -> Self {
        Self::Format(e)
    }
}

/// The two buffers a copy streams through, each as long as one read.
pub struct CopyBuffers<'b> {
    /// Where a ranged read lands.
    pub read: &'b mut [u8],
    /// What a write pulls from.
    pub write: &'b mut [u8],
}

/// Restores indices of shallow snapshots under one deployment's base.
pub struct SnapshotRestorer {
    pub(crate) store: Arc<dyn BlobStore>,
    pub(crate) base: String,
}

impl SnapshotRestorer {
    /// A restorer over `store`, for the deployment at `base`.
    #[must_use]
    pub fn new(store: Arc<dyn BlobStore>, base: &str) -> Self {
        Self {
            store,
            // ALLOC: one base path, once a node.
            base: base.to_owned(),
        }
    }

    /// Creates `target` with `uuid` from `source`, then copies and lands
    /// each shard. The caller records `target` as created before calling,
    /// so a failure anywhere, including in the descriptor's create, is
    /// cleaned up by deleting it.
    ///
    /// Cost: a descriptor create, then for each captured file one write and
    /// one ranged read for each buffer of its bytes, and one manifest
    /// create for each shard. No LIST.
    ///
    /// # Errors
    ///
    /// [`RestoreError::Unsupported`] for what [`RestoreIndex::refusal`]
    /// names; [`RestoreError::Conflict`] when a shard's manifest is taken;
    /// otherwise the failing step's error.
    #[allow(clippy::too_many_arguments)]
    pub fn restore_index(
        &self,
        descriptors: &DescriptorStore,
        source: &RestoreIndex,
        target: &str,
        uuid: &str,
        now_millis: i64,
        record_buf: &mut [u8],
        buffers: &mut CopyBuffers<'_>,
    ) -> Result<(), RestoreError> {
        if let Some(why) = source.refusal()? {
            return Err(RestoreError::Unsupported(why));
        }
        self.restore_with(
            descriptors,
            source,
            target,
            uuid,
            now_millis,
            record_buf,
            |me| {
                for shard in &source.shards {
                    me.restore_shard(source, shard, target, uuid, buffers, &|| false)?;
                }
                Ok(())
            },
        )
    }

    /// Creates the descriptor marked, runs `copy` to land every shard, then
    /// unmarks: the order every restore keeps.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn restore_with(
        &self,
        descriptors: &DescriptorStore,
        source: &RestoreIndex,
        target: &str,
        uuid: &str,
        now_millis: i64,
        record_buf: &mut [u8],
        copy: impl FnOnce(&Self) -> Result<(), RestoreError>,
    ) -> Result<(), RestoreError> {
        let generation =
            self.create_marked(descriptors, source, target, uuid, now_millis, record_buf)?;
        copy(self)?;
        Self::unmark(
            descriptors,
            source,
            target,
            uuid,
            now_millis,
            generation,
            record_buf,
        )
    }

    /// The first half of a restore: creates the descriptor of `target`
    /// marked (a node that dies before the last shard lands leaves an index
    /// every read and write refuses, not one that answers from shards that
    /// are still empty) and returns its generation for [`Self::finish_index`].
    /// Cost: one descriptor create.
    ///
    /// # Errors
    ///
    /// [`RestoreError::Unsupported`] for what [`RestoreIndex::refusal`]
    /// names; [`RestoreError::Meta`] when the name is taken or the store fails.
    pub fn create_marked(
        &self,
        descriptors: &DescriptorStore,
        source: &RestoreIndex,
        target: &str,
        uuid: &str,
        now_millis: i64,
        record_buf: &mut [u8],
    ) -> Result<u64, RestoreError> {
        if let Some(why) = source.refusal()? {
            return Err(RestoreError::Unsupported(why));
        }
        let marked = mark_restoring(source.settings.as_deref())?;
        let mut record = restored_record(source, target, uuid, now_millis);
        record.settings = &marked;
        Ok(descriptors.create(&record, now_millis, record_buf)?)
    }

    /// The second half: copies each shard of `source` into the index
    /// [`Self::create_marked`] made, lands its manifest and clears the mark.
    /// `cancel` is asked before each file; true stops with
    /// [`RestoreError::Cancelled`], the index left marked. `name` is the
    /// target's name and uuid; `made` the creation time and generation.
    ///
    /// # Errors
    ///
    /// As [`Self::restore_index`], and [`RestoreError::Cancelled`].
    #[allow(clippy::too_many_arguments)]
    pub fn finish_index(
        &self,
        descriptors: &DescriptorStore,
        source: &RestoreIndex,
        name: (&str, &str),
        made: (i64, u64),
        record_buf: &mut [u8],
        buffers: &mut CopyBuffers<'_>,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), RestoreError> {
        let ((target, uuid), (now_millis, generation)) = (name, made);
        for shard in &source.shards {
            self.restore_shard(source, shard, target, uuid, buffers, cancel)?;
        }
        Self::unmark(
            descriptors,
            source,
            target,
            uuid,
            now_millis,
            generation,
            record_buf,
        )
    }

    /// Writes the index's own settings back over the marked ones: one swap
    /// from the create's generation, and if a change got there first (an
    /// alias, say) the same swap from what that left.
    #[allow(clippy::too_many_arguments)]
    fn unmark(
        descriptors: &DescriptorStore,
        source: &RestoreIndex,
        target: &str,
        uuid: &str,
        now_millis: i64,
        generation: u64,
        record_buf: &mut [u8],
    ) -> Result<(), RestoreError> {
        let own = source.settings.as_deref().unwrap_or(EMPTY_SETTINGS);
        let mut record = restored_record(source, target, uuid, now_millis);
        record.settings = own;
        if descriptors
            .update(&record, generation, record_buf)?
            .is_some()
        {
            return Ok(());
        }
        // ALLOC: one buffer, only when a change got there first, as large as the record buffer.
        let mut read = vec![0u8; record_buf.len()];
        for _ in 0..UNMARK_ATTEMPTS {
            let held = descriptors.resolve(target, &mut read)?;
            let Target::Index(mut current) = held.target else {
                return Err(RestoreError::Unsupported(
                    "the index was deleted while it was restored",
                ));
            };
            // The name may be a new index by now, made after a delete: leave it be.
            if current.uuid != uuid || !oss_format::is_restoring(current.settings) {
                return Err(RestoreError::Unsupported(
                    "the index was replaced while it was restored",
                ));
            }
            current.settings = own;
            if descriptors
                .update(&current, held.generation, record_buf)?
                .is_some()
            {
                return Ok(());
            }
        }
        Err(RestoreError::Meta(MetaError::Contended))
    }

    fn restore_shard(
        &self,
        source: &RestoreIndex,
        shard: &RestoreShard,
        target: &str,
        uuid: &str,
        buffers: &mut CopyBuffers<'_>,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), RestoreError> {
        let from = ShardRef {
            index: &source.name,
            uuid: &source.uuid,
            shard: shard.shard,
        };
        let to = ShardRef {
            index: target,
            uuid,
            shard: shard.shard,
        };
        let whole = FileLocation::Whole {
            term_dir: FIRST_TERM_DIR,
        };
        // ALLOC: one entry a captured file, at most the manifest's file bound.
        let mut entries = Vec::with_capacity(shard.files.len());
        for (name, recorded) in &shard.files {
            if cancel() {
                return Err(RestoreError::Cancelled);
            }
            let location = FileLocation::parse(recorded)?;
            let (key, offset) = from.blob_key(&self.base, name, &location)?;
            let length = length_of(shard, name, &location)?;
            let (dest, _) = to.blob_key(&self.base, name, &whole)?;
            let mut body = RangedSource {
                store: self.store.as_ref(),
                key: &key,
                start: offset,
                length,
                done: 0,
                scratch: &mut *buffers.read,
            };
            self.store.put(&dest, &mut body, &mut *buffers.write)?;
            entries.push(ManifestEntry {
                name,
                location: whole.encode(),
                length,
            });
        }
        let manifest = encode_manifest(1, None, &entries, -1, -1)?;
        let landed = self
            .store
            .create_register(&to.manifest_key(&self.base)?, &manifest)?;
        match landed {
            SwapOutcome::Applied(_) => Ok(()),
            SwapOutcome::Conflict(_) => Err(RestoreError::Conflict { shard: shard.shard }),
        }
    }
}

/// The descriptor a restore of `source` as `target` starts from, with the
/// settings still to be set.
pub(crate) fn restored_record<'a>(
    source: &'a RestoreIndex,
    target: &'a str,
    uuid: &'a str,
    now_millis: i64,
) -> IndexRecord<'a> {
    let mut record = IndexRecord::new(target, uuid, source.number_of_shards);
    record.mapping = source.mapping.as_deref().map(Cow::Borrowed);
    record.created_at_millis = now_millis;
    record
}

/// How many times the unmarking swap is tried against changes that race it.
const UNMARK_ATTEMPTS: usize = 4;

/// The length the record states for `name`, or its location's range.
fn length_of(
    shard: &RestoreShard,
    name: &str,
    location: &FileLocation<'_>,
) -> Result<u64, RestoreError> {
    shard
        .lengths
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, length)| *length)
        .or_else(|| location.range().map(|(_, length)| length))
        .ok_or(RestoreError::Unsupported(
            "a captured file has no recorded length",
        ))
}

#[cfg(test)]
mod tests;

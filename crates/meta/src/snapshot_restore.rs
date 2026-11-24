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

use oss_blob::{BlobError, BlobResult, BlobStore, ByteRange, ChunkSource, SwapOutcome};
use oss_format::{
    FileLocation, FormatError, IndexRecord, ManifestEntry, RestoreIndex, RestoreShard, ShardRef,
    encode_manifest,
};

use crate::descriptors::DescriptorStore;
use crate::error::MetaError;

/// The term container a restored shard's files are written to: the new
/// index's first term.
const FIRST_TERM_DIR: &str = "t=1";

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
    store: Arc<dyn BlobStore>,
    base: String,
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
        let mut record = IndexRecord::new(target, uuid, source.number_of_shards);
        record.mapping = source.mapping.as_deref().map(Cow::Borrowed);
        record.created_at_millis = now_millis;
        if let Some(settings) = &source.settings {
            record.settings = settings;
        }
        descriptors.create(&record, now_millis, record_buf)?;
        for shard in &source.shards {
            self.restore_shard(source, shard, target, uuid, buffers)?;
        }
        Ok(())
    }

    fn restore_shard(
        &self,
        source: &RestoreIndex,
        shard: &RestoreShard,
        target: &str,
        uuid: &str,
        buffers: &mut CopyBuffers<'_>,
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

/// A range of one blob as a PUT's body: each fill is one ranged read of at
/// most the scratch buffer.
struct RangedSource<'a> {
    store: &'a dyn BlobStore,
    key: &'a str,
    start: u64,
    length: u64,
    done: u64,
    scratch: &'a mut [u8],
}

impl ChunkSource for RangedSource<'_> {
    fn len(&self) -> u64 {
        self.length
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        let left = self.length.saturating_sub(self.done);
        let want = usize::try_from(left).unwrap_or(usize::MAX).min(buf.len());
        if want == 0 {
            return Ok(0);
        }
        let range = ByteRange::From {
            start: self.start.saturating_add(self.done),
            len: u64::try_from(want).unwrap_or(u64::MAX),
        };
        let mut filled = 0usize;
        let mut sink = |chunk: &[u8]| -> BlobResult<()> {
            let end = filled.saturating_add(chunk.len());
            buf.get_mut(filled..end)
                .ok_or(BlobError::Stream("a read returned more than asked"))?
                .copy_from_slice(chunk);
            filled = end;
            Ok(())
        };
        self.store
            .get(self.key, range, &mut *self.scratch, &mut sink)?;
        if filled != want {
            return Err(BlobError::Stream(
                "a captured file is shorter than recorded",
            ));
        }
        self.done = self
            .done
            .saturating_add(u64::try_from(want).unwrap_or(u64::MAX));
        Ok(want)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        self.done = 0;
        Ok(())
    }
}

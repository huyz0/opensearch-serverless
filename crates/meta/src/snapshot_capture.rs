//! Describing an index and reading each shard's published commit, for a
//! snapshot's record, as the shell's `SnapshotOperations.capture` does.
//!
//! The commit read is the one a point in time freezes (a shard's own
//! register, or a bucketed member's entry in its bucket's manifest), copied
//! as the record's `term`, `files` and `lengths`; the unpublished log tail is
//! not part of a snapshot, as in the Java.

use oss_blob::BlobStore;
use oss_format::{
    CommitManifest, IndexCapture, ShardCapture, ShardRef, bucket_manifest_key, parse_bucket_member,
};

use crate::descriptors::{DescriptorStore, Target};
use crate::error::MetaError;
use crate::pit::{CaptureError, live_units};
use crate::snapshot_take::SnapshotTaker;

/// An index as the take resolved it: what the record names, and what is
/// needed to read its shards and to check it was not replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescribedIndex {
    /// The index's name.
    pub name: String,
    /// Its uuid.
    pub uuid: String,
    /// The slots it has, which the record states as its shard count.
    pub number_of_shards: u32,
    /// Its mapping, when it has one.
    pub mapping: Option<String>,
    /// Its settings as a JSON object, when it has any.
    pub settings: Option<Vec<u8>>,
    /// When it was created, in epoch milliseconds; 0 when not recorded.
    pub created_at_millis: i64,
    bucket: Option<String>,
    units: Vec<u32>,
}

impl DescribedIndex {
    /// How many shards the record will list.
    #[must_use]
    pub fn shard_count(&self) -> usize {
        self.units.len()
    }

    /// The record entry for this index with `shards` captured (none while
    /// the name is only reserved).
    #[must_use]
    pub fn capture(&self, shards: Vec<ShardCapture>, with_content: bool) -> IndexCapture {
        IndexCapture {
            name: self.name.clone(),
            uuid: self.uuid.clone(),
            number_of_shards: self.number_of_shards,
            mapping: self.mapping.clone().filter(|_| with_content),
            settings: self.settings.clone().filter(|_| with_content),
            shards,
        }
    }

    /// Whether the index's shard lives in a bucket's shared packs.
    #[must_use]
    pub fn bucketed(&self) -> bool {
        self.bucket.is_some()
    }

    /// The shard numbers to read.
    #[must_use]
    pub fn units(&self) -> &[u32] {
        &self.units
    }
}

impl SnapshotTaker {
    /// Resolves `index` to its incarnation. Cost: one GET.
    ///
    /// # Errors
    ///
    /// [`CaptureError::NoSuchIndex`], [`CaptureError::IsAlias`] and
    /// [`CaptureError::Unsupported`] for an index whose live units are not
    /// all its slots (the Java's own record parser refuses such a record).
    pub fn describe(
        &self,
        descriptors: &DescriptorStore,
        index: &str,
        buf: &mut [u8],
    ) -> Result<DescribedIndex, CaptureError> {
        match descriptors.resolve(index, buf)?.target {
            Target::Index(record) => {
                if oss_format::is_restoring(record.settings) {
                    return Err(CaptureError::Restoring);
                }
                let units = if record.bucket.is_some() {
                    // ALLOC: the one shard, once a take.
                    vec![0]
                } else {
                    live_units(record.unit_layout, record.number_of_shards)?
                };
                let slots = usize::try_from(record.number_of_shards).unwrap_or(usize::MAX);
                if units.len() != slots {
                    return Err(CaptureError::Unsupported(
                        "the index has retired or pending shards",
                    ));
                }
                // ALLOC: the index's names and settings, once a take.
                Ok(DescribedIndex {
                    name: index.to_owned(),
                    uuid: record.uuid.into_owned(),
                    number_of_shards: record.number_of_shards,
                    mapping: record.mapping.map(std::borrow::Cow::into_owned),
                    // ALLOC: the settings, at most the descriptor's size, once a take.
                    settings: Some(record.settings.to_vec()).filter(|s| !s.is_empty()),
                    created_at_millis: record.created_at_millis,
                    bucket: record.bucket.map(std::borrow::Cow::into_owned),
                    units,
                })
            }
            Target::Alias(_) => Err(CaptureError::IsAlias),
            Target::Absent => Err(CaptureError::NoSuchIndex),
        }
    }

    /// Checks `index` is still the incarnation (uuid and bucket) described.
    /// Cost: one GET.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Replaced`] when it was deleted or created again.
    pub fn verify(
        &self,
        descriptors: &DescriptorStore,
        index: &DescribedIndex,
        buf: &mut [u8],
    ) -> Result<(), CaptureError> {
        let still = match descriptors.resolve(&index.name, buf)?.target {
            Target::Index(record) => {
                record.uuid == index.uuid.as_str()
                    && record.bucket.as_deref() == index.bucket.as_deref()
            }
            Target::Alias(_) | Target::Absent => false,
        };
        if still {
            Ok(())
        } else {
            Err(CaptureError::Replaced)
        }
    }

    /// Reads one shard's published commit. Cost: one GET.
    ///
    /// # Errors
    ///
    /// [`CaptureError::NothingPublished`] for a shard with no commit.
    pub fn capture_shard(
        &self,
        index: &DescribedIndex,
        shard: u32,
        buf: &mut [u8],
    ) -> Result<ShardCapture, CaptureError> {
        let store: &dyn BlobStore = &*self.store;
        let (key, member) = match &index.bucket {
            Some(bucket) => (
                bucket_manifest_key(&self.base, bucket).map_err(MetaError::Format)?,
                true,
            ),
            None => (
                ShardRef {
                    index: &index.name,
                    uuid: &index.uuid,
                    shard,
                }
                .manifest_key(&self.base)
                .map_err(MetaError::Format)?,
                false,
            ),
        };
        let Some(read) = store.read_register(&key, buf).map_err(MetaError::Blob)? else {
            return Err(CaptureError::NothingPublished(shard));
        };
        let value = buf.get(..read.len).unwrap_or_default();
        if !member {
            let manifest = CommitManifest::parse(value).map_err(MetaError::Format)?;
            return Ok(shard_capture(shard, &manifest));
        }
        let Some(bucket) = &index.bucket else {
            return Err(CaptureError::Replaced);
        };
        // ALLOC: the member's name and commit, at most the register's size, once a take.
        let entry = parse_bucket_member(value, &index.uuid).map_err(MetaError::Format)?;
        let Some(member) = entry.member.filter(|_| entry.bucket == *bucket) else {
            return Err(CaptureError::Replaced);
        };
        let Some(commit) = member.commit else {
            return Err(CaptureError::NothingPublished(shard));
        };
        let manifest = CommitManifest::parse(&commit).map_err(MetaError::Format)?;
        Ok(shard_capture(shard, &manifest))
    }
}

fn shard_capture(shard: u32, manifest: &CommitManifest<'_>) -> ShardCapture {
    let files = manifest.files();
    ShardCapture {
        shard,
        term: manifest.term,
        // ALLOC: one entry a file, at most MAX_FILES.
        files: files
            .iter()
            .map(|f| (f.name.to_string(), f.location.encode()))
            // ALLOC: the entries counted above.
            .collect(),
        // ALLOC: the entries counted above.
        lengths: files
            .iter()
            .filter_map(|f| f.length.map(|n| (f.name.to_string(), n)))
            // ALLOC: the entries counted above.
            .collect(),
    }
}

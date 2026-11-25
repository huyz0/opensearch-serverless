//! Where every file of a shard's latest commit lives, found with two GETs
//! and no LIST.
//!
//! Ported from the shell's read path for a published commit: the descriptor
//! read of `DescriptorStore.get`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/metadata/DescriptorStore.java`),
//! then `SegmentPublisher.readManifest` for the shard's commit register
//! (`serverless/shell/src/main/java/org/opensearch/serverless/store/SegmentPublisher.java`),
//! with `oss-format` holding the decoders. Unlike the Java, nothing is
//! downloaded here: the result maps each Lucene file to a blob range, and the
//! engine reads ranges as queries need them.

use std::collections::BTreeMap;

use std::sync::Arc;

use oss_blob::{BlobStore, ByteRange, PooledStore};
use oss_format::{
    CommitManifest, DescriptorRecord, FormatError, IndexDescriptor, Register, ShardRef,
    bucket_manifest_key, descriptor_key, parse_bucket_member,
};

use crate::commit_push::{NewestSource, Stage, newer_commit};
use crate::error::ReaderError;
use crate::shared_mapping::SharedMapping;

/// The largest register blob (descriptor or commit manifest) the reader
/// reads. The shell's manifests list at most 16,384 files of about 100
/// bytes each.
pub const MAX_REGISTER_BYTES: usize = 4 << 20;

/// One Lucene file of a commit: a range of one blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRange {
    /// The blob holding the file.
    pub key: String,
    /// Where the file starts in the blob.
    pub offset: u64,
    /// The file's length.
    pub length: u64,
}

/// What a search needs to know of the index, read from its descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexMeta {
    /// The index's uuid, which shard failures name.
    pub uuid: String,
    /// How many shards the index has.
    pub number_of_shards: u32,
    /// The index's fields; empty when the descriptor holds no mapping.
    /// Indexes with the same mapping text share one.
    pub mapping: Arc<SharedMapping>,
    /// The index's settings, flat by dotted key; empty when the descriptor
    /// holds none.
    pub settings: oss_format::ClusterSettings,
}

/// Every file of one shard's latest published commit, by Lucene file name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShardFiles {
    /// The commit's primary term.
    pub term: u64,
    stage: Stage,
    files: BTreeMap<String, FileRange>,
}

impl ShardFiles {
    /// Reads the descriptor of `index` and the commit register of `shard`
    /// under `base`, and resolves every file: exactly two GETs.
    pub fn open<S: BlobStore>(
        store: &PooledStore<S>,
        base: &str,
        index: &str,
        shard: u32,
    ) -> Result<Self, ReaderError> {
        Self::open_with_meta(store, base, index, shard).map(|(files, _)| files)
    }

    /// [`Self::open`], with the index's metadata from the same descriptor
    /// read.
    pub fn open_with_meta<S: BlobStore>(
        store: &PooledStore<S>,
        base: &str,
        index: &str,
        shard: u32,
    ) -> Result<(Self, IndexMeta), ReaderError> {
        Self::open_from(store, base, index, shard, None, None)
    }

    /// [`Self::open_with_meta`] for a commit the shard's writer pushed:
    /// `pushed` is the commit register's value, so only the descriptor is
    /// read, one GET.
    pub fn open_pushed_with_meta<S: BlobStore>(
        store: &PooledStore<S>,
        base: &str,
        index: &str,
        shard: u32,
        pushed: &[u8],
    ) -> Result<(Self, IndexMeta), ReaderError> {
        Self::open_from(store, base, index, shard, Some(pushed), None)
    }

    /// [`Self::open_with_meta`], on the newer of the register's commit and
    /// the one `newest` has from the register's writer: the same two GETs.
    pub fn open_asking_with_meta<S: BlobStore>(
        store: &PooledStore<S>,
        base: &str,
        index: &str,
        shard: u32,
        newest: &dyn NewestSource,
    ) -> Result<(Self, IndexMeta), ReaderError> {
        Self::open_from(store, base, index, shard, None, Some(newest))
    }

    fn open_from<S: BlobStore>(
        store: &PooledStore<S>,
        base: &str,
        index: &str,
        shard: u32,
        pushed: Option<&[u8]>,
        newest: Option<&dyn NewestSource>,
    ) -> Result<(Self, IndexMeta), ReaderError> {
        let descriptor_bytes =
            read_register(store, &descriptor_key(base, index)?, MAX_REGISTER_BYTES)?;
        let descriptor = index_of(&descriptor_bytes)?;
        let meta = meta_of(&descriptor)?;
        let bucket = descriptor.bucket.as_deref();
        let shard_ref = match bucket {
            Some(_) if shard != 0 => return Err(not_followable("no such shard")),
            Some(_) => ShardRef {
                index: &descriptor.name,
                uuid: &descriptor.uuid,
                shard: 0,
            },
            None => descriptor.shard(shard)?,
        };
        let held;
        let register_bytes;
        let value = match (pushed, bucket) {
            (Some(value), _) => value,
            (None, Some(bucket)) => {
                match member_commit(store, base, bucket, &descriptor.uuid)? {
                    Some(commit) => held = commit,
                    // The bucket's entry says nothing was ever committed.
                    None => return Ok((Self::never_written(), meta)),
                }
                held.as_slice()
            }
            (None, None) => {
                match read_register(store, &shard_ref.manifest_key(base)?, MAX_REGISTER_BYTES) {
                    Ok(bytes) => register_bytes = bytes,
                    // The index was created and nothing was published.
                    Err(ReaderError::Blob(oss_blob::BlobError::NotFound))
                        if never_activated(store, base, &descriptor, shard)? =>
                    {
                        return Ok((Self::never_written(), meta));
                    }
                    Err(e) => return Err(e),
                }
                Register::parse(&register_bytes)?.value
            }
        };
        let asked;
        let value = match (pushed, newest) {
            (None, Some(source)) => {
                asked = newer_commit(value, |writer| {
                    source.newest(index, &meta.uuid, shard, writer)
                });
                asked.as_deref().unwrap_or(value)
            }
            _ => value,
        };
        let manifest = CommitManifest::parse(value)?;
        let extents = shard_ref.resolve(base, &manifest)?;
        // ALLOC: one entry a commit file, at most the manifest's file limit.
        let mut files = BTreeMap::new();
        for extent in extents {
            // ALLOC: the name, only for a manifest that lacks a length.
            let length = extent
                .length
                .ok_or_else(|| ReaderError::UnknownLength(extent.name.to_string()))?;
            files.insert(
                // ALLOC: one name a file, at most oss_format::manifest_format::MAX_FILES.
                extent.name.into_owned(),
                FileRange {
                    key: extent.key,
                    offset: extent.offset,
                    length,
                },
            );
        }
        Ok((
            Self {
                term: manifest.term,
                stage: Stage::of(&manifest),
                files,
            },
            meta,
        ))
    }

    /// The commit of a shard that never held a document: one engine file of
    /// no segments, held in memory, so a search of it finds no hits and
    /// reads no object.
    fn never_written() -> Self {
        // ALLOC: one entry, the empty commit's file.
        let mut files = BTreeMap::new();
        if let Some((name, bytes)) = crate::empty_commit::file() {
            files.insert(
                // ALLOC: one name, a first open of a never-written shard.
                name.clone(),
                FileRange {
                    // ALLOC: one key, with the name above.
                    key: crate::empty_commit::INLINE_KEY.to_owned(),
                    offset: 0,
                    length: bytes.len() as u64,
                },
            );
        }
        Self {
            term: 0,
            stage: Stage::new(0, None, -1),
            files,
        }
    }

    /// A commit assembled from `term` and its resolved `files`.
    pub(crate) fn assembled(term: u64, stage: Stage, files: BTreeMap<String, FileRange>) -> Self {
        Self { term, stage, files }
    }

    /// A commit of `files`, for tests.
    #[cfg(test)]
    pub(crate) fn of(term: u64, files: BTreeMap<String, FileRange>) -> Self {
        Self {
            term,
            stage: Stage::new(term, None, -1),
            files,
        }
    }

    /// Where the commit sits among the writer's: its term, writer, stage
    /// and Lucene generation.
    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// The commit's files, in name order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// Where `name` lives, if the commit has it.
    pub fn get(&self, name: &str) -> Option<&FileRange> {
        self.files.get(name)
    }

    /// How many files the commit has.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether the commit has no files.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// The index a descriptor register holds.
pub(crate) fn index_of(register: &[u8]) -> Result<IndexDescriptor<'_>, ReaderError> {
    match DescriptorRecord::parse(Register::parse(register)?.value)? {
        DescriptorRecord::Index(d) => Ok(d),
        DescriptorRecord::Tombstone => Err(ReaderError::Deleted),
        DescriptorRecord::Alias => Err(ReaderError::Alias),
    }
}

/// What a search needs of the index `descriptor` describes.
pub(crate) fn meta_of(descriptor: &IndexDescriptor<'_>) -> Result<IndexMeta, ReaderError> {
    let mapping = crate::shared_mapping::shared_of(descriptor.mapping_escaped)?;
    if descriptor.settings.is_some_and(oss_format::is_restoring) {
        return Err(ReaderError::Restoring);
    }
    let settings = match descriptor.settings {
        Some(bytes) => oss_format::ClusterSettings::parse(bytes)?,
        None => oss_format::ClusterSettings::default(),
    };
    Ok(IndexMeta {
        // ALLOC: one uuid an open index.
        uuid: descriptor.uuid.to_string(),
        number_of_shards: descriptor.number_of_shards,
        mapping,
        settings,
    })
}

fn not_followable(reason: &'static str) -> ReaderError {
    ReaderError::Format(FormatError::NotFollowable {
        what: "bucket member",
        reason,
    })
}

/// The commit a bucketed index's entry in its bucket's manifest holds, none
/// before its first publish: one GET of the manifest register.
fn member_commit<S: BlobStore>(
    store: &PooledStore<S>,
    base: &str,
    bucket: &str,
    uuid: &str,
) -> Result<Option<Vec<u8>>, ReaderError> {
    let bytes = read_register(
        store,
        &bucket_manifest_key(base, bucket)?,
        MAX_REGISTER_BYTES,
    )?;
    let entry = parse_bucket_member(Register::parse(&bytes)?.value, uuid)?;
    Ok(entry
        .member
        .ok_or_else(|| not_followable("the index is not a member of its bucket"))?
        .commit)
}

/// Whether no writer ever took shard `shard` of the index `descriptor`
/// describes: its head is absent, or was left by an earlier index of the
/// name. A shard that was taken and has no commit register lost it, which
/// is an error, not an empty shard. One GET, only when the register is
/// missing.
fn never_activated<S: BlobStore>(
    store: &PooledStore<S>,
    base: &str,
    descriptor: &IndexDescriptor<'_>,
    shard: u32,
) -> Result<bool, ReaderError> {
    let key = oss_cluster::heads::shard_head_key(base, &descriptor.name, shard);
    match read_register(store, &key, MAX_REGISTER_BYTES) {
        Ok(bytes) => {
            let head = oss_format::ShardHead::parse(Register::parse(&bytes)?.value)?;
            Ok(head
                .index_uuid
                .as_deref()
                .is_some_and(|uuid| uuid != descriptor.uuid))
        }
        Err(ReaderError::Blob(oss_blob::BlobError::NotFound)) => Ok(true),
        Err(e) => Err(e),
    }
}

/// Reads a whole register blob, refusing one over `max` bytes.
pub(crate) fn read_register<S: BlobStore>(
    store: &PooledStore<S>,
    key: &str,
    max: usize,
) -> Result<Vec<u8>, ReaderError> {
    // ALLOC: one register, at most `max` bytes, checked as it fills.
    let mut out = Vec::new();
    let mut too_large = false;
    store
        .get(key, ByteRange::Full, &mut |chunk: &[u8]| {
            if out.len().saturating_add(chunk.len()) > max {
                too_large = true;
                return Err(oss_blob::BlobError::Stream("register too large"));
            }
            out.extend_from_slice(chunk);
            Ok(())
        })
        .map_err(|e| {
            if too_large {
                ReaderError::RegisterTooLarge
            } else {
                e.into()
            }
        })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use oss_blob::{Emulator, SliceSource};
    use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool};

    use super::*;

    fn store_with(key: &str, len: usize) -> PooledStore<Emulator> {
        let config = BudgetConfig::new(1 << 20)
            .with_pool(Pool::Transport, 1 << 20)
            .expect("fits");
        let budget = MemoryBudget::new(&config);
        let buffers = BufferPool::new(&budget, Pool::Transport, 16, 1).expect("fits");
        let store = PooledStore::new(Emulator::new(1 << 20, 1), buffers);
        store
            .put(key, &mut SliceSource::new(&vec![7u8; len]))
            .expect("put");
        store
    }

    #[test]
    fn a_register_at_its_limit_is_read_and_one_byte_more_is_refused() {
        let store = store_with("r", 100);
        assert_eq!(
            read_register(&store, "r", 100).expect("at the limit").len(),
            100
        );
        assert!(matches!(
            read_register(&store, "r", 99),
            Err(ReaderError::RegisterTooLarge)
        ));
        assert!(matches!(
            read_register(&store, "missing", 100),
            Err(ReaderError::Blob(oss_blob::BlobError::NotFound))
        ));
    }

    #[test]
    fn an_empty_commit_is_empty() {
        let files = ShardFiles::of(1, BTreeMap::new());
        assert!(files.is_empty());
        assert_eq!(files.len(), 0);
        let mut one = files.clone();
        one.files.insert(
            "segments_1".into(),
            FileRange {
                key: "k".into(),
                offset: 0,
                length: 1,
            },
        );
        assert!(!one.is_empty());
    }
}

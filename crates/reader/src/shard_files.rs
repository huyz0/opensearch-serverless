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

use oss_blob::{BlobStore, ByteRange, PooledStore};
use oss_format::{CommitManifest, DescriptorRecord, Register, descriptor_key};

use crate::error::ReaderError;
use crate::mapping::Mapping;

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
    pub mapping: Mapping,
}

/// Every file of one shard's latest published commit, by Lucene file name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShardFiles {
    /// The commit's primary term.
    pub term: u64,
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
        let descriptor_bytes =
            read_register(store, &descriptor_key(base, index)?, MAX_REGISTER_BYTES)?;
        let descriptor = match DescriptorRecord::parse(Register::parse(&descriptor_bytes)?.value)? {
            DescriptorRecord::Index(d) => d,
            DescriptorRecord::Tombstone => return Err(ReaderError::Deleted),
            DescriptorRecord::Alias => return Err(ReaderError::Alias),
        };
        let mapping = match descriptor.mapping_escaped {
            Some(escaped) => Mapping::from_escaped(escaped)?,
            None => Mapping::default(),
        };
        let meta = IndexMeta {
            // ALLOC: one uuid an open index.
            uuid: descriptor.uuid.to_string(),
            number_of_shards: descriptor.number_of_shards,
            mapping,
        };
        let shard_ref = descriptor.shard(shard)?;
        let manifest_bytes =
            read_register(store, &shard_ref.manifest_key(base)?, MAX_REGISTER_BYTES)?;
        let manifest = CommitManifest::parse(Register::parse(&manifest_bytes)?.value)?;
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
                files,
            },
            meta,
        ))
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

/// Reads a whole register blob, refusing one over `max` bytes.
fn read_register<S: BlobStore>(
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
        let files = ShardFiles {
            term: 1,
            files: BTreeMap::new(),
        };
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

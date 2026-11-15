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
        let descriptor_bytes = read_register(store, &descriptor_key(base, index)?)?;
        let descriptor = match DescriptorRecord::parse(Register::parse(&descriptor_bytes)?.value)? {
            DescriptorRecord::Index(d) => d,
            DescriptorRecord::Tombstone => return Err(ReaderError::Deleted),
            DescriptorRecord::Alias => return Err(ReaderError::Alias),
        };
        let shard_ref = descriptor.shard(shard)?;
        let manifest_bytes = read_register(store, &shard_ref.manifest_key(base)?)?;
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
        Ok(Self {
            term: manifest.term,
            files,
        })
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

/// Reads a whole register blob, refusing one over [`MAX_REGISTER_BYTES`].
fn read_register<S: BlobStore>(store: &PooledStore<S>, key: &str) -> Result<Vec<u8>, ReaderError> {
    // ALLOC: one register, at most MAX_REGISTER_BYTES, checked as it fills.
    let mut out = Vec::new();
    let mut too_large = false;
    store
        .get(key, ByteRange::Full, &mut |chunk: &[u8]| {
            if out.len().saturating_add(chunk.len()) > MAX_REGISTER_BYTES {
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

//! A bucket's manifest register: read, create and swap.
//!
//! Ported from `BucketStore` (`read`, `readMember`, `swap`, `create`).

use std::sync::Arc;

use oss_blob::register::ABSENT_GENERATION;
use oss_blob::{BlobError, BlobStore, SwapOutcome};
use oss_format::{
    BucketEntry, BucketManifest, FormatError, ManifestCodec, bucket_manifest_key, bucket_open_key,
    encode_open, parse_bucket_member, parse_open,
};

use super::BucketError;

/// What a register read has room for first: some thousand members' commits.
const READ_FIRST: usize = 256 << 10;

/// The most a register read keeps: a manifest is at most about 2 MiB when
/// members are admitted, and a little more as they checkpoint.
const READ_BYTES: usize = 8 << 20;

/// What an open-bucket register read has room for: a generation in decimal.
const OPEN_READ_BYTES: usize = 64;

/// How a manifest is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestWrite {
    /// JSON, which every node reads.
    Json,
    /// The compact form, compressed with the codec.
    Compact(ManifestCodec),
    /// The compact form, compressed with the codec only when that pays.
    Auto(ManifestCodec),
}

impl ManifestWrite {
    fn encode(self, manifest: &BucketManifest) -> Result<Vec<u8>, FormatError> {
        match self {
            Self::Json => manifest.encode(),
            Self::Compact(codec) => manifest.encode_compact(codec),
            Self::Auto(codec) => manifest.encode_compact_auto(codec),
        }
    }
}

/// A manifest as read, with the generation to swap against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketRead {
    /// The manifest.
    pub manifest: BucketManifest,
    /// The register's generation.
    pub generation: u64,
    /// The size of the register's value as stored, in bytes.
    pub stored_bytes: usize,
}

/// A manifest as swapped in: its new generation and stored size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Swapped {
    /// The register's new generation.
    pub generation: u64,
    /// The size of the value written, in bytes.
    pub stored_bytes: usize,
}

/// A stripe's open-bucket register as read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenRead {
    /// The generation of the stripe's buckets that takes new members.
    pub bucket_generation: u64,
    /// The register's generation, to swap against.
    pub generation: u64,
}

/// The buckets of one deployment, in an object store.
pub struct BucketStore {
    store: Arc<dyn BlobStore>,
    base: String,
    write: ManifestWrite,
}

impl std::fmt::Debug for BucketStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BucketStore")
            .field("base", &self.base)
            .field("write", &self.write)
            .finish_non_exhaustive()
    }
}

impl BucketStore {
    /// The buckets under `base` (empty, or ending in `/`), written in the
    /// form `write` names.
    #[must_use]
    pub fn new(store: Arc<dyn BlobStore>, base: &str, write: ManifestWrite) -> Self {
        Self {
            store,
            // ALLOC: one base, once a store.
            base: base.to_owned(),
            write,
        }
    }

    /// The register's bytes and generation, or `None` when there is none.
    fn read_raw(&self, bucket: &str) -> Result<Option<(Vec<u8>, u64)>, BucketError> {
        let key = bucket_manifest_key(&self.base, bucket)?;
        for size in [READ_FIRST, READ_BYTES] {
            // ALLOC: one buffer a register read, at most READ_BYTES: a node's first read of a bucket, or after a lost swap.
            let mut buf = vec![0u8; size];
            match self.store.read_register(&key, &mut buf) {
                Ok(Some(read)) => {
                    buf.truncate(read.len);
                    return Ok(Some((buf, read.generation)));
                }
                Ok(None) => return Ok(None),
                // A larger manifest than this buffer: once more with room for the most.
                Err(BlobError::InvalidArgument(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Err(FormatError::TooLarge {
            what: "bucket manifest",
            limit: READ_BYTES as u64,
            actual: 0,
        }
        .into())
    }

    /// Reads a bucket's manifest whole, or `None` when it has none.
    ///
    /// # Errors
    ///
    /// The store's failures; [`BucketError::Format`] for a register that is
    /// not a manifest.
    pub fn read(&self, bucket: &str) -> Result<Option<BucketRead>, BucketError> {
        let Some((bytes, generation)) = self.read_raw(bucket)? else {
            return Ok(None);
        };
        Ok(Some(BucketRead {
            manifest: BucketManifest::parse(&bytes)?,
            generation,
            stored_bytes: bytes.len(),
        }))
    }

    /// Reads one member's entry without parsing the other members: `None`
    /// when the bucket has no register, and the entry's `member` is `None`
    /// when the index is not in it.
    ///
    /// # Errors
    ///
    /// As [`BucketStore::read`].
    pub fn read_member(
        &self,
        bucket: &str,
        index_uuid: &str,
    ) -> Result<Option<BucketEntry>, BucketError> {
        let Some((bytes, _)) = self.read_raw(bucket)? else {
            return Ok(None);
        };
        Ok(Some(parse_bucket_member(&bytes, index_uuid)?))
    }

    /// Creates the bucket's register with `manifest`: `true` when it did,
    /// `false` when the bucket already existed, which is left as it is.
    ///
    /// # Errors
    ///
    /// The store's failures; [`BucketError::Format`] for a manifest that
    /// cannot be written.
    pub fn create(&self, manifest: &BucketManifest) -> Result<bool, BucketError> {
        let key = bucket_manifest_key(&self.base, manifest.bucket())?;
        let bytes = self.write.encode(manifest)?;
        Ok(matches!(
            self.store.create_register(&key, &bytes)?,
            SwapOutcome::Applied(_)
        ))
    }

    /// Replaces the bucket's manifest if its register is at `generation`:
    /// the new generation and size, or `None` when another writer moved it
    /// first.
    ///
    /// # Errors
    ///
    /// The store's failures; [`BucketError::Format`] for a manifest that
    /// cannot be written.
    pub fn swap(
        &self,
        manifest: &BucketManifest,
        generation: u64,
    ) -> Result<Option<Swapped>, BucketError> {
        let key = bucket_manifest_key(&self.base, manifest.bucket())?;
        let bytes = self.write.encode(manifest)?;
        let outcome = if generation == ABSENT_GENERATION {
            self.store.create_register(&key, &bytes)?
        } else {
            self.store.swap_register(&key, generation, &bytes)?
        };
        Ok(match outcome {
            SwapOutcome::Applied(next) => Some(Swapped {
                generation: next,
                stored_bytes: bytes.len(),
            }),
            SwapOutcome::Conflict(_) => None,
        })
    }

    /// Reads a stripe's open-bucket register, or `None` when it was never
    /// written (generation 0 of the stripe's buckets is open).
    ///
    /// # Errors
    ///
    /// The store's failures; [`BucketError::Format`] for a value that is not
    /// a generation.
    pub fn read_open(&self, stripe: usize) -> Result<Option<OpenRead>, BucketError> {
        let key = bucket_open_key(&self.base, stripe)?;
        // ALLOC: one small buffer, a generation in decimal.
        let mut buf = vec![0u8; OPEN_READ_BYTES];
        match self.store.read_register(&key, &mut buf) {
            Ok(Some(read)) => Ok(Some(OpenRead {
                bucket_generation: parse_open(buf.get(..read.len).unwrap_or_default())?,
                generation: read.generation,
            })),
            Ok(None) => Ok(None),
            Err(BlobError::InvalidArgument(_)) => Err(FormatError::TooLarge {
                what: "open bucket",
                limit: OPEN_READ_BYTES as u64,
                actual: 0,
            }
            .into()),
            Err(e) => Err(e.into()),
        }
    }

    /// Moves a stripe to bucket generation `bucket_generation` if its
    /// register is at `expected_generation` ([`ABSENT_GENERATION`] for none,
    /// which creates it): `false` when another writer moved it first.
    ///
    /// # Errors
    ///
    /// The store's failures.
    pub fn swap_open(
        &self,
        stripe: usize,
        bucket_generation: u64,
        expected_generation: u64,
    ) -> Result<bool, BucketError> {
        let key = bucket_open_key(&self.base, stripe)?;
        let bytes = encode_open(bucket_generation);
        let outcome = if expected_generation == ABSENT_GENERATION {
            self.store.create_register(&key, &bytes)?
        } else {
            self.store
                .swap_register(&key, expected_generation, &bytes)?
        };
        Ok(matches!(outcome, SwapOutcome::Applied(_)))
    }
}

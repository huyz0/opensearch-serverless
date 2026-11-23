//! Picking the bucket a new index joins, and recording it there.
//!
//! Ported from `BucketAssigner` (huyz0/OpenSearch `feature/serverlessshell`).
//!
//! Indices are spread over a fixed number of stripes by their group (a
//! tenant, say), so a tenant's indices made around the same time share
//! buckets and creates in different stripes never contend. Each stripe has
//! one open bucket, named by a generation in the stripe's register; a full
//! bucket moves the stripe on to the next generation. A bucket is full by
//! data size: a bucket of a few large indices and one of many tiny ones both
//! stop at the same bytes. The member count and the stored manifest size are
//! ceilings only. Joining is one compare-and-swap of the bucket's manifest,
//! and is done before the descriptor names the bucket: the manifest is the
//! truth, so an index it does not list is never served as a member.

use oss_blob::register::ABSENT_GENERATION;
use oss_cluster::routing::murmur3_x64_h1;
use oss_format::{BucketManifest, BucketMember, MAX_BUCKET_MEMBERS};

use super::{BucketError, BucketStore};

/// How many stripes new indices are spread over.
pub const STRIPES: usize = 16;

/// Lost races before a join or leave gives up.
const ATTEMPTS: usize = 16;

/// When a bucket takes no new members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    target_bytes: u64,
    max_members: usize,
    max_manifest_bytes: usize,
}

impl Limits {
    /// Limits that keep every bound positive, and `max_members` within what
    /// a manifest holds.
    ///
    /// `target_bytes` is the data size that fills a bucket, `max_members`
    /// the most members one holds whatever its size, and
    /// `max_manifest_bytes` the stored manifest size past which it takes no
    /// more.
    #[must_use]
    pub fn new(target_bytes: u64, max_members: usize, max_manifest_bytes: usize) -> Self {
        Self {
            target_bytes: target_bytes.max(1),
            max_members: max_members.clamp(1, MAX_BUCKET_MEMBERS),
            max_manifest_bytes: max_manifest_bytes.max(1),
        }
    }

    /// The data size that fills a bucket.
    #[must_use]
    pub fn target_bytes(&self) -> u64 {
        self.target_bytes
    }

    /// The most members one bucket holds.
    #[must_use]
    pub fn max_members(&self) -> usize {
        self.max_members
    }

    /// The stored manifest size past which a bucket takes no more.
    #[must_use]
    pub fn max_manifest_bytes(&self) -> usize {
        self.max_manifest_bytes
    }

    fn full(&self, manifest: &BucketManifest, stored_bytes: usize) -> Result<bool, BucketError> {
        Ok(manifest.len() >= self.max_members
            || manifest.data_bytes()? >= self.target_bytes
            || stored_bytes >= self.max_manifest_bytes)
    }
}

/// Assigns new indices to buckets.
#[derive(Debug)]
pub struct BucketAssigner {
    store: BucketStore,
    limits: Limits,
}

impl BucketAssigner {
    /// An assigner over a deployment's buckets.
    #[must_use]
    pub fn new(store: BucketStore, limits: Limits) -> Self {
        Self { store, limits }
    }

    /// The stripe a group's indices go to: the low four bits of the group's
    /// 64-bit hash (the shell's `floorMod(h1, 16)`).
    #[must_use]
    pub fn stripe(group: &str) -> usize {
        // A floorMod by 16 of a two's complement long is its low four bits.
        let low = murmur3_x64_h1(group.as_bytes()) & 0xF;
        usize::try_from(low).unwrap_or(0)
    }

    /// A stripe's bucket of one generation: `s<stripe>-<generation>`.
    #[must_use]
    pub fn bucket_id(stripe: usize, generation: u64) -> String {
        // ALLOC: one id an assignment.
        format!("s{stripe:02}-{generation}")
    }

    /// Adds an index to its group's open bucket and returns the bucket.
    ///
    /// # Errors
    ///
    /// The store's failures; [`BucketError::Busy`] when every attempt lost a
    /// race.
    pub fn join(
        &self,
        group: &str,
        index_name: &str,
        index_uuid: &str,
    ) -> Result<String, BucketError> {
        let stripe = Self::stripe(group);
        for _ in 0..ATTEMPTS {
            let open = self.store.read_open(stripe)?;
            let generation = open.map_or(0, |o| o.bucket_generation);
            let bucket = Self::bucket_id(stripe, generation);
            let read = self.store.read(&bucket)?;
            let (current, stored_bytes, expected) = match read {
                Some(read) => (read.manifest, read.stored_bytes, read.generation),
                None => (BucketManifest::empty(&bucket)?, 0, ABSENT_GENERATION),
            };
            if current.member(index_uuid).is_some() {
                return Ok(bucket);
            }
            if self.limits.full(&current, stored_bytes)? {
                // Full: move the stripe on, whoever gets there first. A lost swap means another create already did.
                self.store.swap_open(
                    stripe,
                    generation.saturating_add(1),
                    open.map_or(ABSENT_GENERATION, |o| o.generation),
                )?;
                continue;
            }
            let joined = current.with_member(index_uuid, BucketMember::first(index_name, None)?)?;
            if self.store.swap(&joined, expected)?.is_some() {
                return Ok(bucket);
            }
        }
        Err(BucketError::Busy { attempts: ATTEMPTS })
    }

    /// Takes an index back out of a bucket it joined, for a create that
    /// failed after joining. Best effort: a member left behind has no
    /// descriptor naming the bucket, and is never served.
    ///
    /// # Errors
    ///
    /// The store's failures.
    pub fn leave(&self, bucket: &str, index_uuid: &str) -> Result<(), BucketError> {
        for _ in 0..ATTEMPTS {
            let Some(read) = self.store.read(bucket)? else {
                return Ok(());
            };
            if read.manifest.member(index_uuid).is_none() {
                return Ok(());
            }
            let without = read.manifest.without_member(index_uuid);
            if self.store.swap(&without, read.generation)?.is_some() {
                return Ok(());
            }
        }
        Ok(())
    }
}

//! Bucket membership from index descriptors.
//!
//! Ported from `DescriptorBucketHeads` (huyz0/OpenSearch
//! `feature/serverlessshell`). A descriptor names its bucket, so every node
//! answers alike from what it reads anyway to route the shard. The answer is
//! kept for an hour per index incarnation: a node checks the heads of what it
//! holds every few minutes, and a descriptor read per member per check would
//! cost more than the bucket saves. An index's bucket does not change for an
//! incarnation except by promotion out of it, which [`forget`] drops.
//!
//! [`forget`]: DescriptorBucketHeads::forget

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use super::bucket::{BucketHeads, Member};
use crate::error::ClusterError;
use crate::lease::LeaseClock;

/// How long a member's bucket is answered from an earlier read.
pub const MEMBERSHIP_CACHE_MILLIS: i64 = 3_600_000;

/// What an index's descriptor says of its bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Described {
    /// The index's uuid.
    pub uuid: String,
    /// The bucket it lives in, if any.
    pub bucket: Option<String>,
}

/// Reads a descriptor the way routing does, from a recent read when there is
/// one.
pub trait Descriptors: Send + Sync {
    /// An index's uuid and bucket, or `None` when it has no descriptor.
    ///
    /// # Errors
    ///
    /// The descriptor cannot be read.
    fn describe(&self, index: &str) -> Result<Option<Described>, ClusterError>;
}

/// The shards a node holds open: `(index, shard, index uuid)`.
pub type OpenShards = Box<dyn Fn() -> Vec<(String, u32, String)> + Send + Sync>;

#[derive(Debug, Clone)]
struct Known {
    uuid: String,
    bucket: Option<String>,
    read_at: i64,
}

/// Answers bucket membership from descriptors, and which members a node
/// holds open from that node's own shards.
pub struct DescriptorBucketHeads {
    descriptors: Arc<dyn Descriptors>,
    clock: Arc<dyn LeaseClock>,
    open_by_node: Mutex<HashMap<String, OpenShards>>,
    known: Mutex<HashMap<String, Known>>,
}

impl DescriptorBucketHeads {
    /// A resolver reading descriptors through `descriptors`, aging its
    /// answers by `clock`.
    #[must_use]
    pub fn new(descriptors: Arc<dyn Descriptors>, clock: Arc<dyn LeaseClock>) -> Self {
        Self {
            descriptors,
            clock,
            // ALLOC: one entry per node registered, in-process nodes only.
            open_by_node: Mutex::default(),
            // ALLOC: one entry per index asked about on a held shard.
            known: Mutex::default(),
        }
    }

    fn known(&self) -> std::sync::MutexGuard<'_, HashMap<String, Known>> {
        self.known.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records an index's bucket from a descriptor a caller has just read,
    /// so the head operation that follows costs no second read.
    pub fn note(&self, index: &str, uuid: &str, bucket: Option<&str>) {
        let read_at = self.clock.now_millis();
        // ALLOC: one entry per index, replaced in place.
        self.known().insert(
            index.to_owned(),
            Known {
                // ALLOC: one uuid and bucket per entry.
                uuid: uuid.to_owned(),
                // ALLOC: as above.
                bucket: bucket.map(str::to_owned),
                read_at,
            },
        );
    }

    /// Drops what is known of an index's bucket, for an index deleted or
    /// moved out of its bucket.
    pub fn forget(&self, index: &str) {
        self.known().remove(index);
    }

    /// Registers what `node_id` holds open, read on each release of a member.
    pub fn register(&self, node_id: &str, open: OpenShards) {
        self.open_by_node
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            // ALLOC: one entry per node, replaced in place.
            .insert(node_id.to_owned(), open);
    }
}

impl BucketHeads for DescriptorBucketHeads {
    fn bucket_of(
        &self,
        index: &str,
        shard: u32,
        index_uuid: Option<&str>,
    ) -> Result<Option<Member>, ClusterError> {
        if shard != 0 || index.starts_with('#') {
            return Ok(None);
        }
        let now = self.clock.now_millis();
        let cached = self.known().get(index).cloned();
        let differs = |entry: &Known| index_uuid.is_some_and(|u| u != entry.uuid);
        let entry = match cached {
            Some(entry)
                if now.saturating_sub(entry.read_at) < MEMBERSHIP_CACHE_MILLIS
                    && !differs(&entry) =>
            {
                entry
            }
            _ => {
                let Some(described) = self.descriptors.describe(index)? else {
                    self.forget(index);
                    return Ok(None);
                };
                let entry = Known {
                    uuid: described.uuid,
                    bucket: described.bucket,
                    read_at: now,
                };
                // ALLOC: one entry per index, replaced in place.
                self.known().insert(index.to_owned(), entry.clone());
                entry
            }
        };
        if differs(&entry) {
            return Ok(None);
        }
        match &entry.bucket {
            Some(bucket) => Member::new(bucket, &entry.uuid).map(Some),
            None => Ok(None),
        }
    }

    fn others_open_on(
        &self,
        bucket_id: &str,
        node_id: &str,
        except_index: &str,
    ) -> Result<bool, ClusterError> {
        let open = {
            let nodes = self
                .open_by_node
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match nodes.get(node_id) {
                Some(open) => open(),
                None => return Ok(false),
            }
        };
        for (index, shard, uuid) in open {
            if shard == 0
                && index != except_index
                && self
                    .bucket_of(&index, 0, Some(&uuid))?
                    .is_some_and(|m| m.bucket_id() == bucket_id)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

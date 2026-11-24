//! The node's bucket wiring: who a bucket's members are, and where a
//! member's commit is recorded.
//!
//! A shard whose index descriptor names a bucket is owned through the
//! bucket's head and publishes its commit as its entry in the bucket's
//! manifest. Built only when `serverless.buckets.enabled` is on.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_cluster::ClusterError;
use oss_cluster::LeaseClock;
use oss_cluster::heads::{BucketHeads, Described, DescriptorBucketHeads, Descriptors};
use oss_meta::DescriptorStore;
use oss_writer::bucket::{
    BucketAssigner, BucketCommits, BucketStore, Limits, ManifestWrite, MemberRegister, policy,
};
use oss_writer::publish::CommitRegister;

use super::WriterRole;

/// The bytes a descriptor read has room for.
const DESCRIPTOR_BYTES: usize = 64 << 10;

/// Descriptors read from the descriptor store.
struct StoreDescriptors(Arc<DescriptorStore>);

impl Descriptors for StoreDescriptors {
    fn describe(&self, index: &str) -> Result<Option<Described>, ClusterError> {
        // ALLOC: one descriptor's bytes a read, at most once an index an hour.
        let mut buf = vec![0u8; DESCRIPTOR_BYTES];
        let record = self
            .0
            .get(index, &mut buf)
            .map_err(|_| ClusterError::Heads("a descriptor could not be read"))?;
        Ok(record.map(|r| Described {
            // ALLOC: one uuid and bucket id a read.
            uuid: r.uuid.to_string(),
            // ALLOC: as above.
            bucket: r.bucket.as_deref().map(str::to_owned),
        }))
    }
}

/// What a node needs to own and publish bucket members.
pub struct BucketWiring {
    commits: Arc<BucketCommits>,
    heads: Arc<DescriptorBucketHeads>,
    assigner: BucketAssigner,
}

impl BucketWiring {
    /// The wiring over the buckets under `base`, written in the form
    /// `write` names, membership read from `descriptors`, new members
    /// placed by `limits`.
    #[must_use]
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        descriptors: Arc<DescriptorStore>,
        clock: Arc<dyn LeaseClock>,
        write: ManifestWrite,
        limits: Limits,
    ) -> Self {
        // ALLOC: once a node.
        let assigner =
            BucketAssigner::new(BucketStore::new(Arc::clone(&store), base, write), limits);
        let commits = Arc::new(BucketCommits::new(BucketStore::new(store, base, write)));
        let heads = Arc::new(DescriptorBucketHeads::new(
            Arc::new(StoreDescriptors(descriptors)),
            clock,
        ));
        Self {
            commits,
            heads,
            assigner,
        }
    }

    /// What the shard head store routes members through.
    #[must_use]
    pub fn heads(&self) -> Arc<dyn BucketHeads> {
        Arc::clone(&self.heads) as Arc<dyn BucketHeads>
    }

    /// Adds a new one-shard index to its group's open bucket and returns the
    /// bucket id its descriptor names.
    ///
    /// # Errors
    ///
    /// The bucket store's failure, as text.
    pub fn join(&self, index_name: &str, index_uuid: &str) -> Result<String, String> {
        self.assigner
            .join(policy::group(index_name, None), index_name, index_uuid)
            .map_err(|e| e.to_string())
    }

    /// Tells the bucket heads that `node_id` holds open what `role` holds,
    /// so giving back a member keeps the bucket while a sibling is open.
    pub fn register_open(&self, node_id: &str, role: Arc<WriterRole>) {
        self.heads.register(
            node_id,
            Box::new(move || {
                // ALLOC: one entry a shard held, at most the shards of the node.
                role.held_shards()
                    .into_iter()
                    .map(|h| (h.name, h.shard, h.uuid))
                    .collect()
            }),
        );
    }

    /// Where the shard `shard` of the index `index` (incarnation `uuid`)
    /// records its commit: its bucket entry when its descriptor names a
    /// bucket, `None` for a shard of its own.
    ///
    /// # Errors
    ///
    /// The descriptor could not be read.
    pub fn register(
        &self,
        index: &str,
        shard: u32,
        uuid: &str,
    ) -> Result<Option<Arc<dyn CommitRegister>>, String> {
        let member = self
            .heads
            .bucket_of(index, shard, Some(uuid))
            .map_err(|e| e.to_string())?;
        Ok(member.map(|m| {
            // ALLOC: one register a member shard's publisher.
            Arc::new(MemberRegister::new(
                Arc::clone(&self.commits),
                m.bucket_id(),
                m.index_uuid(),
            )) as Arc<dyn CommitRegister>
        }))
    }
}

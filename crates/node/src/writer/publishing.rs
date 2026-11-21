//! The publish pass of the writer role: which held shards are due, and the
//! publisher each one's commits go through.
//!
//! Ported from the publish pass of the shell's `BackgroundReconciler`
//! (`lazyPublishDue`, `SegmentPublisher.publish`): a shard's commit is
//! published when the lazy policy says so, not on every write.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use oss_blob::BlobStore;
use oss_format::ShardRef;
use oss_writer::publish::ShardPublisher;
use oss_writer::publish::lazy::{LazyPublish, Policy, PublishAge};

/// A shard by index uuid and number.
pub(super) type Key = (String, u32);

/// What one publish pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PublishPass {
    /// Shards published.
    pub published: usize,
    /// Shards that were due and failed, by `[uuid][shard]` and the reason;
    /// each is due again at the next pass.
    pub failed: Vec<String>,
}

/// How a role publishes: where, as whom, and when a shard is due.
pub struct Publishing {
    store: Arc<dyn BlobStore>,
    base: String,
    node_id: String,
    clock: Box<dyn Fn() -> u64 + Send + Sync>,
    lazy: Mutex<LazyPublish<Key>>,
    publishers: Mutex<HashMap<Key, ShardPublisher>>,
    /// Held from taking a commit's snapshot to its publish, so commits are
    /// published in the order they were taken.
    gate: Mutex<()>,
    ages: Mutex<HashMap<Key, PublishAge>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Publishing {
    /// Publishing into `store` under the cluster `base`, as node `node_id`,
    /// by `policy`, read against `clock` (milliseconds).
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        node_id: &str,
        policy: Policy,
        clock: Box<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            store,
            // ALLOC: two names, once a node.
            base: base.to_owned(),
            node_id: node_id.to_owned(),
            clock,
            lazy: Mutex::new(LazyPublish::new(policy)),
            // ALLOC: one publisher a shard this node has published.
            publishers: Mutex::new(HashMap::new()),
            gate: Mutex::new(()),
            // ALLOC: one entry a shard of an index that sets a refresh interval.
            ages: Mutex::new(HashMap::new()),
        }
    }

    /// Held across taking a snapshot and publishing it.
    pub(super) fn gate(&self) -> MutexGuard<'_, ()> {
        lock(&self.gate)
    }

    /// A write reached `key`.
    pub(super) fn written(&self, key: &Key) {
        let now = (self.clock)();
        lock(&self.lazy).mark_written(key.clone(), now);
    }

    /// `key` was released or opened again: it is due once more.
    pub(super) fn forget(&self, key: &Key) {
        lock(&self.lazy).forget(key);
        lock(&self.publishers).remove(key);
        lock(&self.ages).remove(key);
    }

    /// How long `key`'s index lets a write wait, from its settings.
    pub(super) fn set_age(&self, key: &Key, age: PublishAge) {
        let mut ages = lock(&self.ages);
        if age == PublishAge::Node {
            ages.remove(key);
        } else {
            // ALLOC: one entry a shard.
            ages.insert(key.clone(), age);
        }
    }

    /// Whether `key`, written through `max_seq_no`, is due now.
    pub(super) fn due(&self, key: &Key, max_seq_no: i64) -> bool {
        let now = (self.clock)();
        let age = lock(&self.ages)
            .get(key)
            .copied()
            .unwrap_or(PublishAge::Node);
        lock(&self.lazy).due(key, max_seq_no, age, now)
    }

    /// Whether `key`, written through `max_seq_no`, holds writes no commit
    /// of it covers.
    pub(super) fn unpublished(&self, key: &Key, max_seq_no: i64) -> bool {
        lock(&self.lazy).unpublished(key, max_seq_no)
    }

    /// `key` was published through `max_seq_no`.
    pub(super) fn published(&self, key: &Key, max_seq_no: i64) {
        lock(&self.lazy).published(key.clone(), max_seq_no);
    }

    /// The publisher of `key`, made when it has none.
    ///
    /// # Errors
    ///
    /// The index name, uuid or base not being part of a key.
    pub(super) fn with_publisher<T>(
        &self,
        key: &Key,
        index: &str,
        run: impl FnOnce(&mut ShardPublisher, &str, u64) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut publishers = lock(&self.publishers);
        if !publishers.contains_key(key) {
            let shard = ShardRef {
                index,
                uuid: &key.0,
                shard: key.1,
            };
            let made = ShardPublisher::new(Arc::clone(&self.store), &self.base, &shard)
                .map_err(|e| e.to_string())?;
            // ALLOC: one publisher a shard.
            publishers.insert(key.clone(), made);
        }
        let publisher = publishers
            .get_mut(key)
            .ok_or_else(|| String::from("no publisher"))?;
        run(publisher, &self.node_id, (self.clock)())
    }
}

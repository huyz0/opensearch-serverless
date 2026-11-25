//! The publish pass of the writer role: which held shards are due, and the
//! publisher each one's commits go through.
//!
//! Ported from the publish pass of the shell's `BackgroundReconciler`
//! (`lazyPublishDue`, `SegmentPublisher.publish`): a shard's commit is
//! published when the lazy policy says so, not on every write.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use oss_blob::BlobStore;
use oss_format::ShardRef;
use oss_writer::publish::ShardPublisher;

use super::BucketWiring;
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
    /// One publisher a shard, each behind its own lock: publishes of
    /// different shards run together.
    publishers: Mutex<HashMap<Key, Arc<Mutex<ShardPublisher>>>>,
    /// Held from taking a commit's snapshot to its publish, so commits are
    /// published in the order they were taken.
    gate: Mutex<()>,
    ages: Mutex<HashMap<Key, PublishAge>>,
    buckets: Option<Arc<BucketWiring>>,
    /// How many times longer a shard's writes may wait, by index uuid.
    stretch: OnceLock<Stretch>,
}

/// By how much a shard's publish age is stretched: its index uuid in, the
/// multiplier (1 for none) out.
pub type Stretch = Arc<dyn Fn(&str) -> u64 + Send + Sync>;

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
            buckets: None,
            stretch: OnceLock::new(),
        }
    }

    /// Records the commits of the shards whose descriptors name a bucket in
    /// their bucket's manifest.
    #[must_use]
    pub fn with_buckets(mut self, buckets: Arc<BucketWiring>) -> Self {
        self.buckets = Some(buckets);
        self
    }

    /// Installs how far a shard's publish age is stretched. False when one
    /// is installed already.
    pub(super) fn set_stretch(&self, stretch: Stretch) -> bool {
        self.stretch.set(stretch).is_ok()
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

    /// `key` was released or opened again: it is due once more. A publish of
    /// the dropped publisher still in flight is waited for, as it was when one
    /// lock covered every publisher, so a publisher made next never overlaps it.
    pub(super) fn forget(&self, key: &Key) {
        lock(&self.lazy).forget(key);
        let dropped = lock(&self.publishers).remove(key);
        if let Some(dropped) = dropped {
            // Held and let go: waits for a publish under way.
            drop(lock(&dropped));
        }
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
        let stretch = self.stretch.get().map_or(1, |f| f(&key.0));
        lock(&self.lazy).due_stretched(key, max_seq_no, age, stretch, now)
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
        let known = lock(&self.publishers).get(key).map(Arc::clone);
        let publisher = match known {
            Some(publisher) => publisher,
            None => {
                // Made outside the map's lock: a bucketed shard reads its
                // register to make one.
                let shard = ShardRef {
                    index,
                    uuid: &key.0,
                    shard: key.1,
                };
                let mut made = ShardPublisher::new(Arc::clone(&self.store), &self.base, &shard)
                    .map_err(|e| e.to_string())?;
                if let Some(buckets) = &self.buckets
                    && let Some(register) = buckets.register(index, key.1, &key.0)?
                {
                    made.commit_register(register);
                }
                // A publisher another caller made first is the one kept.
                // ALLOC: one publisher a shard.
                let made = Arc::new(Mutex::new(made));
                Arc::clone(lock(&self.publishers).entry(key.clone()).or_insert(made))
            }
        };
        let mut publisher = lock(&publisher);
        run(&mut publisher, &self.node_id, (self.clock)())
    }
}

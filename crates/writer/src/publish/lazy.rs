//! When a shard is due for a publish: by the age of its oldest unpublished
//! write, or by how many operations it holds beyond its last publish.
//!
//! A publish is a segment blob and a register swap, so a shard taking a write
//! every few hundred milliseconds must not pay them on each. An acknowledged
//! write is already in the node's log; what a publish buys is reader
//! freshness and a shorter log to replay, and both are well served by
//! publishing a busy shard about once an age. Nothing here makes a request.
//!
//! Ported from `DirtyPublishing.lazyPublishDue` and
//! `BackgroundReconciler.setLazyPublish` (huyz0/OpenSearch
//! `feature/serverlessshell`).

use std::collections::BTreeMap;

use oss_types::settings::{LongSetting, Scope, Setting};

/// `serverless.wal.node_log.publish_max_age_millis`: how old a shard's oldest
/// unpublished write may get, for an index that sets no `refresh_interval`.
/// Zero publishes every shard with a write, whatever its index's
/// `refresh_interval` (the Java turns lazy publish off at zero). Default 1,000.
pub const PUBLISH_MAX_AGE_MILLIS: LongSetting = LongSetting::new(
    "serverless.wal.node_log.publish_max_age_millis",
    1_000,
    0,
    i64::MAX,
    Scope::Node,
    "With one write log per node, how long a write may wait to be published, in milliseconds: \
     the longest a write takes to be searchable, and what bounds how much of the log is kept. \
     0 publishes every shard with a write at each pass.",
);

/// `serverless.publish.max_unpublished_operations`: operations beyond its last
/// publish that make a shard due before its age. Zero never publishes early.
/// Default 10,000.
pub const PUBLISH_MAX_UNPUBLISHED_OPERATIONS: LongSetting = LongSetting::new(
    "serverless.publish.max_unpublished_operations",
    10_000,
    0,
    i64::MAX,
    Scope::Node,
    "How many operations beyond its last publish make a shard due for a publish before its \
     age; 0 for never early. Bounds what a successor replays from the log.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    PUBLISH_MAX_AGE_MILLIS.setting(),
    PUBLISH_MAX_UNPUBLISHED_OPERATIONS.setting(),
];

/// How a shard's index bounds the age of its unpublished writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishAge {
    /// The index sets no `refresh_interval`: the node's age.
    Node,
    /// The index's `refresh_interval`, in milliseconds.
    Millis(u64),
    /// `refresh_interval` of `-1`: published by operations or on release only.
    Never,
}

/// The node's lazy publish settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    max_age_millis: u64,
    max_operations: u64,
    tick_millis: u64,
}

impl Policy {
    /// A policy of the node's age (0 is eager), operation bound (0 never
    /// early) and tick, the write log's window: a due time is rounded up to
    /// a multiple of it. A tick of 0 is read as 1.
    #[must_use]
    pub fn new(max_age_millis: u64, max_operations: u64, tick_millis: u64) -> Self {
        Self {
            max_age_millis,
            max_operations,
            tick_millis: tick_millis.max(1),
        }
    }
}

/// What the tracker remembers of one shard.
#[derive(Clone, Copy, Debug, Default)]
struct ShardState {
    /// The highest sequence number a publish covered; none if never published.
    published_through: Option<i64>,
    /// When its oldest unpublished write was reported.
    unpublished_since: Option<u64>,
}

/// Which shards are due for a publish, by the policy. Callers serialise its
/// use; keys are whatever names a shard.
#[derive(Debug)]
pub struct LazyPublish<K: Ord + Clone> {
    policy: Policy,
    shards: BTreeMap<K, ShardState>,
}

impl<K: Ord + Clone> LazyPublish<K> {
    /// A tracker under `policy`, knowing no shard.
    #[must_use]
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            // ALLOC: one entry per open shard of the node, dropped by `forget` on close.
            shards: BTreeMap::new(),
        }
    }

    /// A write reached `shard` at `now_millis`. Only the oldest unpublished
    /// write's time is kept.
    pub fn mark_written(&mut self, shard: K, now_millis: u64) {
        self.shards
            .entry(shard)
            .or_default()
            .unpublished_since
            .get_or_insert(now_millis);
    }

    /// Whether `shard`, whose highest sequence number is `max_seq_no`
    /// (negative for none), is due for a publish at `now_millis`.
    ///
    /// A shard whose writes were never marked is aged from the first ask that
    /// finds it has some.
    pub fn due(&mut self, shard: &K, max_seq_no: i64, age: PublishAge, now_millis: u64) -> bool {
        self.due_stretched(shard, max_seq_no, age, 1, now_millis)
    }

    /// [`Self::due`] with the age a write may wait multiplied by `stretch`
    /// (1 leaves it): a tenant over its budget publishes less often. The
    /// operation count still bounds the wait.
    pub fn due_stretched(
        &mut self,
        shard: &K,
        max_seq_no: i64,
        age: PublishAge,
        stretch: u64,
        now_millis: u64,
    ) -> bool {
        let through = self.shards.get(shard).and_then(|s| s.published_through);
        if max_seq_no < 0 || through.is_some_and(|t| max_seq_no <= t) {
            if let Some(state) = self.shards.get_mut(shard) {
                state.unpublished_since = None;
            }
            return false;
        }
        let Some(through) = through else {
            // Never published by this node: once, so a new index is
            // searchable at once and its commit records where this node's
            // writes begin.
            return true;
        };
        if self.policy.max_age_millis == 0 {
            return true;
        }
        let since = *self
            .shards
            .entry(shard.clone())
            .or_default()
            .unpublished_since
            .get_or_insert(now_millis);
        let age_millis = match age {
            PublishAge::Node => Some(self.policy.max_age_millis),
            PublishAge::Millis(millis) => Some(millis),
            PublishAge::Never => None,
        };
        if let Some(age_millis) = age_millis {
            let due_at = since
                .saturating_add(age_millis.saturating_mul(stretch.max(1)))
                .div_ceil(self.policy.tick_millis)
                .saturating_mul(self.policy.tick_millis);
            if now_millis >= due_at {
                return true;
            }
        }
        let limit = i64::try_from(self.policy.max_operations).unwrap_or(i64::MAX);
        limit > 0 && max_seq_no.saturating_sub(through) >= limit
    }

    /// Whether `shard`, whose highest sequence number is `max_seq_no`, holds
    /// writes no publish covers: any write at all when it was never
    /// published, else any beyond what the last publish covered.
    #[must_use]
    pub fn unpublished(&self, shard: &K, max_seq_no: i64) -> bool {
        max_seq_no >= 0
            && self
                .shards
                .get(shard)
                .and_then(|s| s.published_through)
                .is_none_or(|through| max_seq_no > through)
    }

    /// A publish of `shard` covering everything to `max_seq_no` completed.
    /// A write marked while it ran lost its stamp with the publish's: it is
    /// aged from the next mark, or from the next ask that finds it.
    pub fn published(&mut self, shard: K, max_seq_no: i64) {
        let state = self.shards.entry(shard).or_default();
        state.published_through = Some(max_seq_no);
        state.unpublished_since = None;
    }

    /// `shard` closed or moved away: it is due once again if it comes back.
    pub fn forget(&mut self, shard: &K) {
        self.shards.remove(shard);
    }
}

#[cfg(test)]
mod tests {
    use oss_types::settings::Registry;

    use super::*;

    const NODE: PublishAge = PublishAge::Node;

    fn lazy(age: u64, ops: u64, tick: u64) -> LazyPublish<&'static str> {
        LazyPublish::new(Policy::new(age, ops, tick))
    }

    // docs/specs/tenant-publish-stretch AC-1
    #[test]
    fn a_stretched_age_waits_that_many_times_longer_and_one_changes_nothing() {
        for (stretch, due_at) in [(1, 1_000), (4, 4_000)] {
            let mut l = lazy(1_000, 0, 250);
            l.published("s", 0);
            l.mark_written("s", 0);
            assert!(
                !l.due_stretched(&"s", 1, NODE, stretch, due_at - 250),
                "{stretch}"
            );
            assert!(l.due_stretched(&"s", 1, NODE, stretch, due_at), "{stretch}");
        }
        // The plain ask is a stretch of one.
        let mut l = lazy(1_000, 0, 250);
        l.published("s", 0);
        l.mark_written("s", 0);
        assert!(l.due(&"s", 1, NODE, 1_000));
    }

    // docs/specs/tenant-publish-stretch AC-1
    #[test]
    fn a_stretch_does_not_lift_the_operation_bound_and_age_zero_is_never_stretched() {
        // Three operations are due at once however long the age is stretched.
        let mut l = lazy(1_000, 3, 250);
        l.published("s", 0);
        l.mark_written("s", 0);
        assert!(!l.due_stretched(&"s", 2, NODE, 4, 10), "under the bound");
        assert!(l.due_stretched(&"s", 3, NODE, 4, 10), "at the bound");
        // A node at age zero publishes every pass, stretched or not; so does
        // an index whose own age is zero.
        let mut l = lazy(0, 0, 250);
        l.published("s", 0);
        assert!(l.due_stretched(&"s", 1, NODE, 4, 0));
        let mut l = lazy(1_000, 0, 250);
        l.published("s", 0);
        l.mark_written("s", 0);
        assert!(l.due_stretched(&"s", 1, PublishAge::Millis(0), 4, 0));
    }

    #[test]
    fn a_shard_is_unpublished_while_it_holds_writes_no_publish_covers() {
        let mut l = lazy(1_000, 0, 250);
        assert!(!l.unpublished(&"s", -1), "no writes");
        assert!(l.unpublished(&"s", 0), "never published");
        l.published("s", 4);
        assert!(!l.unpublished(&"s", 4), "covered");
        assert!(l.unpublished(&"s", 5), "one beyond");
        l.forget(&"s");
        assert!(l.unpublished(&"s", 4), "forgotten is never published");
    }

    /// A tracker whose shard `s` was published through 0 at time 0.
    fn published_once(age: u64, ops: u64, tick: u64) -> LazyPublish<&'static str> {
        let mut l = lazy(age, ops, tick);
        l.mark_written("s", 0);
        l.published("s", 0);
        l
    }

    #[test]
    fn a_shard_never_published_is_due_once_it_has_a_write_and_not_before() {
        let mut l = lazy(1_000, 0, 1);
        assert!(!l.due(&"s", -1, NODE, 5), "no write");
        l.mark_written("s", 5);
        assert!(l.due(&"s", 0, NODE, 5), "first publish at once");
    }

    #[test]
    fn a_shard_is_due_at_its_age_counted_from_its_oldest_unpublished_write() {
        let mut l = published_once(1_000, 0, 1);
        l.mark_written("s", 100);
        l.mark_written("s", 900);
        assert!(!l.due(&"s", 2, NODE, 1_099), "one ms early");
        assert!(
            l.due(&"s", 2, NODE, 1_100),
            "the first write's age, not the last's"
        );
    }

    #[test]
    fn the_due_time_rounds_up_to_the_tick() {
        let mut l = published_once(1_000, 0, 250);
        l.mark_written("s", 10_001);
        assert!(!l.due(&"s", 2, NODE, 11_001));
        assert!(!l.due(&"s", 2, NODE, 11_249));
        assert!(l.due(&"s", 2, NODE, 11_250));
        let mut exact = published_once(1_000, 0, 250);
        exact.mark_written("s", 10_000);
        assert!(exact.due(&"s", 2, NODE, 11_000), "already on a tick");
    }

    #[test]
    fn operations_make_a_shard_due_before_its_age_and_zero_never_does() {
        let mut l = published_once(60_000, 100, 1);
        l.mark_written("s", 10);
        assert!(!l.due(&"s", 99, NODE, 11), "99 beyond the publish");
        assert!(l.due(&"s", 100, NODE, 11), "100 beyond it");
        let mut never = published_once(60_000, 0, 1);
        never.mark_written("s", 10);
        assert!(!never.due(&"s", 1_000_000, NODE, 11));
    }

    #[test]
    fn an_indexs_refresh_interval_replaces_the_nodes_age_and_minus_one_is_never_by_age() {
        let mut l = published_once(60_000, 0, 1);
        l.mark_written("s", 0);
        assert!(!l.due(&"s", 2, PublishAge::Millis(5_000), 4_999));
        assert!(l.due(&"s", 2, PublishAge::Millis(5_000), 5_000));
        let mut never = published_once(1, 50, 1);
        never.mark_written("s", 0);
        assert!(!never.due(&"s", 2, PublishAge::Never, u64::MAX), "no age");
        assert!(
            never.due(&"s", 50, PublishAge::Never, 0),
            "operations still"
        );
    }

    #[test]
    fn after_a_publish_a_shard_waits_for_a_later_write_and_ages_from_it() {
        let mut l = published_once(1_000, 0, 1);
        l.mark_written("s", 100);
        assert!(l.due(&"s", 1, NODE, 1_100));
        l.published("s", 1);
        assert!(!l.due(&"s", 1, NODE, 9_999), "nothing unpublished");
        // A write that follows the publish.
        l.mark_written("s", 5_000);
        assert!(!l.due(&"s", 2, NODE, 5_999));
        assert!(l.due(&"s", 2, NODE, 6_000), "aged from its own mark");
    }

    #[test]
    fn a_write_with_no_mark_is_aged_from_the_first_ask_that_sees_it() {
        let mut l = published_once(1_000, 0, 1);
        assert!(!l.due(&"s", 3, NODE, 4_000), "stamped now");
        assert!(!l.due(&"s", 3, NODE, 4_999));
        assert!(l.due(&"s", 3, NODE, 5_000));
    }

    #[test]
    fn age_zero_is_eager() {
        let mut l = published_once(0, 0, 1);
        assert!(!l.due(&"s", 0, NODE, 1), "nothing unpublished");
        l.mark_written("s", 1);
        assert!(l.due(&"s", 1, NODE, 1), "any write is due");
    }

    #[test]
    fn age_zero_is_eager_whatever_the_indexs_interval() {
        for age in [PublishAge::Millis(5_000), PublishAge::Never] {
            let mut l = published_once(0, 0, 1);
            l.mark_written("s", 1);
            assert!(l.due(&"s", 1, age, 1), "{age:?}");
        }
    }

    #[test]
    fn a_forgotten_shard_is_due_once_again() {
        let mut l = published_once(60_000, 0, 1);
        l.mark_written("s", 1);
        assert!(!l.due(&"s", 1, NODE, 2));
        l.forget(&"s");
        assert!(l.due(&"s", 1, NODE, 2));
    }

    #[test]
    fn the_settings_form_a_valid_registry_with_the_javas_defaults() {
        let registry = Registry::new(&[SETTINGS]).expect("valid");
        assert_eq!(registry.all().count(), 2);
        assert_eq!(PUBLISH_MAX_AGE_MILLIS.default(), 1_000);
        assert_eq!(PUBLISH_MAX_UNPUBLISHED_OPERATIONS.default(), 10_000);
        assert!(PUBLISH_MAX_AGE_MILLIS.parse("-1").is_err());
    }
}

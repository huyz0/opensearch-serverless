//! The `terms` aggregation's request and shard-level result
//! (`TermsAggregationBuilder`, `LongTerms` at OpenSearch 3.10.0), and the
//! reduce of results across shards (`InternalTerms.reduce`).

pub use lucene_aggs::bucket_aggs::TermsOrder;

use crate::aggs::bucket::keyword_options::KeywordOptions;
use crate::aggs::bucket::running;
use crate::aggs::bucket::sub_order::SubOrder;
use crate::aggs::bucket::sub_values::{BucketSubs, reduce_keyed};
use crate::aggs::partial::Value;

/// `BucketUtils.suggestShardSideQueueSize`: the shard size when none is
/// asked for.
pub fn suggested_shard_size(size: usize) -> usize {
    size.saturating_add(size / 2).saturating_add(10)
}

/// What a `terms` aggregation asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermsSpec {
    /// How many buckets the answer keeps.
    pub size: usize,
    /// How many each shard keeps (never below `size`).
    pub shard_size: usize,
    /// The fewest documents a bucket of the answer holds.
    pub min_doc_count: u64,
    /// The fewest documents a bucket a shard sends holds: `shard_min_doc_count`
    /// capped at `min_doc_count` (`BucketCountThresholds.ensureValidity`).
    pub shard_min_doc_count: u64,
    /// How the buckets are ordered.
    pub order: TermsOrder,
    /// The options only a keyword field reads.
    pub keyword: KeywordOptions,
    /// Set when the order is by a metric sub-aggregation: the buckets are
    /// ordered by it, and `order` is not read.
    pub sub_order: Option<SubOrder>,
    /// `show_term_doc_count_error`: each bucket writes its error bound.
    pub show_error: bool,
}

impl Default for TermsSpec {
    /// `TermsAggregationBuilder`'s defaults: ten buckets by count.
    fn default() -> Self {
        Self {
            size: 10,
            shard_size: suggested_shard_size(10),
            min_doc_count: 1,
            shard_min_doc_count: 0,
            order: TermsOrder::CountDesc,
            keyword: KeywordOptions::default(),
            sub_order: None,
            show_error: false,
        }
    }
}

/// What one shard (or one reduce) holds for a `terms`: `LongTerms`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TermsResult {
    /// What the aggregation asked for.
    pub spec: TermsSpec,
    /// The documents of the buckets not kept.
    pub other_doc_count: u64,
    /// `docCountError`: -1 when unbounded.
    pub doc_count_error: i64,
    /// `(key, doc count)`: by key as the order lists keys (ascending unless
    /// ordered by key descending) from a shard, by the order from a reduce.
    pub buckets: Vec<(i64, u64)>,
    /// Each bucket's `doc_count_error_upper_bound` (-1 when unbounded), in
    /// the order of `buckets`; empty unless the spec shows them, when a
    /// shard's are all 0.
    pub bucket_errors: Vec<i64>,
    /// The buckets' sub-aggregations.
    pub subs: BucketSubs,
}

/// `InternalTerms.reduce` as a final reduce: `None` unless there is a part
/// and every part is a `terms`.
pub fn reduce(parts: &[Value]) -> Option<Value> {
    // ALLOC: one reference a shard, bounded by the shards that answered.
    let results: Option<Vec<&TermsResult>> = parts
        .iter()
        .map(|p| match p {
            Value::Terms(t) => Some(t.as_ref()),
            _ => None,
        })
        // ALLOC: one reference a shard, as above.
        .collect();
    let results = results?;
    if results.is_empty() {
        return None;
    }
    let reduced = reduce_results(&results);
    if !running::add(reduced.buckets.len()) {
        return None;
    }
    // ALLOC: one box a `terms`, bounded by the request's.
    Some(Value::Terms(Box::new(reduced)))
}

/// A shard's `shard_min_doc_count`: the buckets under `min` go, with the
/// sub-aggregation lists beside them (`subs` is empty or one list a bucket),
/// and their documents are returned for `other_doc_count`.
pub(super) fn drop_below<K, S>(min: u64, buckets: &mut Vec<(K, u64)>, subs: &mut Vec<S>) -> u64 {
    if min == 0 || buckets.iter().all(|b| b.1 >= min) {
        return 0;
    }
    let aligned = subs.len() == buckets.len();
    // ALLOC: one flag a shard bucket, at most `shard_size`.
    let keep: Vec<bool> = buckets.iter().map(|b| b.1 >= min).collect();
    let dropped = buckets.iter().filter(|b| b.1 < min).map(|b| b.1).sum();
    let mut flags = keep.iter();
    buckets.retain(|_| flags.next().copied().unwrap_or(true));
    if aligned {
        let mut flags = keep.iter();
        subs.retain(|_| flags.next().copied().unwrap_or(true));
    }
    dropped
}

impl TermsResult {
    /// Drops the buckets under the spec's `shard_min_doc_count`, once the
    /// sub-aggregation lists are in place; their documents join the others.
    pub fn apply_shard_min(&mut self) {
        let dropped = drop_below(
            self.spec.shard_min_doc_count,
            &mut self.buckets,
            &mut self.subs.lists,
        );
        self.other_doc_count = self.other_doc_count.saturating_add(dropped);
    }
}

/// `InternalTerms.getDocCountError` of one shard's `terms`: what it may have
/// cut, or -1 when that is unbounded.
pub(super) fn shard_error<K>(spec: &TermsSpec, buckets: &[(K, u64)], reported: i64) -> i64 {
    let sub_ordered = spec.sub_order.is_some();
    let key_ordered =
        !sub_ordered && matches!(spec.order, TermsOrder::KeyAsc | TermsOrder::KeyDesc);
    if buckets.is_empty() || buckets.len() < spec.shard_size || key_ordered {
        return 0;
    }
    match spec.order {
        _ if sub_ordered => -1,
        TermsOrder::CountDesc if reported > 0 => reported,
        TermsOrder::CountDesc => buckets
            .iter()
            .map(|b| i64::try_from(b.1).unwrap_or(i64::MAX))
            .min()
            .unwrap_or(0),
        _ => -1,
    }
}

/// The `doc_count_error_upper_bound` of each of `kept`'s buckets after a
/// reduce (`InternalTerms.reduce` and `reduceBucket`): a part takes its own
/// shard error off the error stored in each bucket it holds, and the sum of
/// every part's error is added back, so a bucket's error is the sum of the
/// errors of the parts without it. `parts` is each part's keys, stored
/// errors (empty for 0) and shard error. Empty unless `show`.
pub(super) fn bucket_errors<K: Ord + Copy>(
    show: bool,
    sum_error: i64,
    parts: &[(Vec<K>, &[i64], i64)],
    kept: &[K],
) -> Vec<i64> {
    if !show {
        // ALLOC: empty.
        return Vec::new();
    }
    // ALLOC: one entry a distinct key of the parts' buckets.
    let mut by_key: std::collections::BTreeMap<K, i64> = std::collections::BTreeMap::new();
    for (keys, stored, error) in parts {
        for (key, at) in keys.iter().zip(0..) {
            let held = stored.get(at).copied().unwrap_or(0);
            let slot = by_key.entry(*key).or_insert(0);
            *slot = if *slot == -1 || held == -1 {
                -1
            } else {
                slot.saturating_add(held).saturating_sub((*error).max(0))
            };
        }
    }
    kept.iter()
        .map(|key| match by_key.get(key) {
            _ if sum_error == -1 => -1,
            Some(-1) => -1,
            Some(held) => held.saturating_add(sum_error),
            None => sum_error,
        })
        // ALLOC: one error a kept bucket, at most `size`.
        .collect()
}

/// The order's comparison of two `(key, doc count)` buckets: the better one
/// is `Less`.
pub(super) fn compare<K: Ord>(order: TermsOrder, a: (&K, u64), b: (&K, u64)) -> std::cmp::Ordering {
    match order {
        TermsOrder::CountDesc => b.1.cmp(&a.1).then(a.0.cmp(b.0)),
        TermsOrder::CountAsc => a.1.cmp(&b.1).then(a.0.cmp(b.0)),
        TermsOrder::KeyAsc => a.0.cmp(b.0),
        TermsOrder::KeyDesc => b.0.cmp(a.0),
    }
}

/// The final reduce of `parts`, which are not empty, of one aggregation
/// (`InternalTerms.reduce`): equal keys add, the buckets under
/// `min_doc_count` go, the rest are ordered and cut at `size`, the cut ones'
/// documents joining `sum_other_doc_count`. It holds one entry a distinct
/// key of the parts.
pub fn reduce_results(parts: &[&TermsResult]) -> TermsResult {
    let spec = parts.first().map(|p| p.spec.clone()).unwrap_or_default();
    let mut other = 0u64;
    let mut sum_error = 0i64;
    // ALLOC: one error a part, bounded by the shards.
    let mut part_errors = Vec::with_capacity(parts.len());
    // ALLOC: one entry a distinct key of the shards' buckets.
    let mut merged: std::collections::BTreeMap<i64, u64> = std::collections::BTreeMap::new();
    for part in parts {
        other = other.saturating_add(part.other_doc_count);
        let error = shard_error(&part.spec, &part.buckets, part.doc_count_error);
        part_errors.push(error);
        sum_error = if sum_error == -1 || error == -1 {
            -1
        } else {
            sum_error.saturating_add(error)
        };
        for &(key, count) in &part.buckets {
            let slot = merged.entry(key).or_insert(0);
            *slot = slot.saturating_add(count);
        }
    }
    // ALLOC: one entry a bucket that passes `min_doc_count`.
    let mut kept: Vec<(i64, u64)> = merged
        .into_iter()
        .filter(|&(_, count)| count >= spec.min_doc_count)
        // ALLOC: as above.
        .collect();
    kept.sort_by(|a, b| compare(spec.order, (&a.0, a.1), (&b.0, b.1)));
    for &(_, count) in kept.iter().skip(spec.size) {
        other = other.saturating_add(count);
    }
    kept.truncate(spec.size);
    let doc_count_error = if sum_error == -1 || parts.len() != 1 {
        sum_error
    } else {
        0
    };
    // ALLOC: one key a kept bucket, at most `size`.
    let keys: Vec<i64> = kept.iter().map(|b| b.0).collect();
    // ALLOC: one key list a shard, each at most `shard_size` keys: shards x shard_size.
    let held: Vec<Vec<i64>> = parts
        .iter()
        .map(|p| p.buckets.iter().map(|b| b.0).collect())
        // ALLOC: as above.
        .collect();
    let shown = parts
        .iter()
        .zip(&part_errors)
        .zip(&held)
        .map(|((p, e), k)| (k.clone(), p.bucket_errors.as_slice(), *e))
        // ALLOC: one entry a shard, as above.
        .collect::<Vec<_>>();
    let bucket_errors = bucket_errors(spec.show_error, sum_error, &shown, &keys);
    let pairs: Vec<(&[i64], &BucketSubs)> = held
        .iter()
        .zip(parts)
        .map(|(k, p)| (k.as_slice(), &p.subs))
        // ALLOC: as above.
        .collect();
    TermsResult {
        spec,
        other_doc_count: other,
        doc_count_error,
        subs: reduce_keyed(&pairs, &keys),
        bucket_errors,
        buckets: kept,
    }
}

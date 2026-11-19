//! The `terms` aggregation's request and shard-level result
//! (`TermsAggregationBuilder`, `LongTerms` at OpenSearch 3.10.0), and the
//! reduce of results across shards (`InternalTerms.reduce`).

pub use lucene_aggs::bucket_aggs::TermsOrder;

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
    /// How the buckets are ordered.
    pub order: TermsOrder,
}

impl Default for TermsSpec {
    /// `TermsAggregationBuilder`'s defaults: ten buckets by count.
    fn default() -> Self {
        Self {
            size: 10,
            shard_size: suggested_shard_size(10),
            min_doc_count: 1,
            order: TermsOrder::CountDesc,
        }
    }
}

/// What one shard (or one reduce) holds for a `terms`: `LongTerms`.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    // ALLOC: one box a `terms`, bounded by the request's.
    Some(Value::Terms(Box::new(reduce_results(&results))))
}

/// `InternalTerms.getDocCountError` of one shard's `terms`: what it may have
/// cut, or -1 when that is unbounded.
fn shard_error(terms: &TermsResult) -> i64 {
    let key_ordered = matches!(terms.spec.order, TermsOrder::KeyAsc | TermsOrder::KeyDesc);
    if terms.buckets.is_empty() || terms.buckets.len() < terms.spec.shard_size || key_ordered {
        return 0;
    }
    match terms.spec.order {
        TermsOrder::CountDesc if terms.doc_count_error > 0 => terms.doc_count_error,
        TermsOrder::CountDesc => terms
            .buckets
            .iter()
            .map(|b| i64::try_from(b.1).unwrap_or(i64::MAX))
            .min()
            .unwrap_or(0),
        _ => -1,
    }
}

/// The order's comparison of two `(key, doc count)` buckets: the better one
/// is `Less`.
fn compare(order: TermsOrder, a: (i64, u64), b: (i64, u64)) -> std::cmp::Ordering {
    match order {
        TermsOrder::CountDesc => b.1.cmp(&a.1).then(a.0.cmp(&b.0)),
        TermsOrder::CountAsc => a.1.cmp(&b.1).then(a.0.cmp(&b.0)),
        TermsOrder::KeyAsc => a.0.cmp(&b.0),
        TermsOrder::KeyDesc => b.0.cmp(&a.0),
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
    // ALLOC: one entry a distinct key of the shards' buckets.
    let mut merged: std::collections::BTreeMap<i64, u64> = std::collections::BTreeMap::new();
    for part in parts {
        other = other.saturating_add(part.other_doc_count);
        let error = shard_error(part);
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
    kept.sort_by(|a, b| compare(spec.order, *a, *b));
    for &(_, count) in kept.iter().skip(spec.size) {
        other = other.saturating_add(count);
    }
    kept.truncate(spec.size);
    let doc_count_error = if sum_error == -1 || parts.len() != 1 {
        sum_error
    } else {
        0
    };
    TermsResult {
        spec,
        other_doc_count: other,
        doc_count_error,
        buckets: kept,
    }
}

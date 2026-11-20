//! Running a `range` on one shard: the engine's `range` node
//! (`RangeAggregator`) and its result as `InternalRange` holds it.

use lucene_aggs::bucket_aggs::AggNode;
use lucene_aggs::metric_aggs::ValueKind;

use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::bucket::range::{RangeBucket, RangeResult, RangeSpec};
use crate::aggs::bucket::terms_collect::too_many_buckets;
use crate::aggs::metric::MetricAgg;
use crate::aggs::partial::Value;
use crate::search::ShardError;

/// `agg`'s `range` options (every `range` has them).
fn spec_of(agg: &MetricAgg) -> RangeSpec {
    agg.range.as_deref().cloned().unwrap_or_default()
}

/// The engine's node for a `range` of the long (or date) field `agg` reads.
pub fn node(agg: &MetricAgg) -> AggNode {
    AggNode::Range {
        // ALLOC: one field name an aggregation, bounded by the request's.
        field: agg.field.clone(),
        missing: None,
        kind: ValueKind::Long,
        // ALLOC: one pair a range, which the shard bounds before it runs.
        ranges: spec_of(agg).ranges.iter().map(|r| (r.from, r.to)).collect(),
        // ALLOC: empty; sub-aggregations are refused.
        subs: Vec::new(),
    }
}

/// `MultiBucketConsumerService`: more ranges than the bucket limit fail the
/// shard.
pub fn check_limit(agg: &MetricAgg) -> Result<(), ShardError> {
    match agg.range.as_deref() {
        Some(spec) if spec.ranges.len() > BUCKET_LIMIT => {
            Err(too_many_buckets(spec.ranges.len(), BUCKET_LIMIT))
        }
        _ => Ok(()),
    }
}

/// `agg`'s result with the document count of range `i` taken from `docs[i]`.
pub fn value(agg: &MetricAgg, docs: &[u64]) -> Value {
    // ALLOC: one box a `range`, bounded by the request's.
    Value::Range(Box::new(from_counts(&spec_of(agg), docs)))
}

/// The result with the document count of range `i` taken from `docs[i]`,
/// none counted where `docs` is short: the engine's result, or no document
/// at all (an unmapped field, or no match).
pub fn from_counts(spec: &RangeSpec, docs: &[u64]) -> RangeResult {
    RangeResult {
        keyed: spec.keyed,
        // ALLOC: one bucket a range, which the shard bounds before it runs.
        buckets: spec
            .ranges
            .iter()
            .enumerate()
            .map(|(i, r)| RangeBucket {
                // ALLOC: one key a range, bounded by the request's.
                key: r.key.clone(),
                from: r.from,
                to: r.to,
                doc_count: docs.get(i).copied().unwrap_or(0),
            })
            // ALLOC: as above.
            .collect(),
    }
}

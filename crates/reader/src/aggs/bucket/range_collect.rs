//! Running a `range` on one shard: the engine's `range` node
//! (`RangeAggregator`) and its result as `InternalRange` holds it.

use lucene_aggs::bucket_aggs::AggNode;
use lucene_aggs::metric_aggs::ValueKind;

use crate::aggs::bucket::range::{RangeBucket, RangeResult, RangeSpec};

/// The engine's node for a `range` of the long (or date) field `field`.
pub fn node(field: &str, spec: &RangeSpec) -> AggNode {
    AggNode::Range {
        // ALLOC: one field name an aggregation, bounded by the request's.
        field: field.to_owned(),
        missing: None,
        kind: ValueKind::Long,
        // ALLOC: one pair a range, which the shard bounds before it runs.
        ranges: spec.ranges.iter().map(|r| (r.from, r.to)).collect(),
        // ALLOC: empty; sub-aggregations are refused.
        subs: Vec::new(),
    }
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

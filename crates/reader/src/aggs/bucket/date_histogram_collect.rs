//! Running a `date_histogram` on one shard: the engine's `date_histogram`
//! node (`DateHistogramAggregator`) and its result as `InternalDateHistogram`
//! holds it.

use lucene_aggs::bucket_aggs::AggNode;

use crate::aggs::bucket::date_histogram::{DateHistogramResult, DateHistogramSpec};
use crate::aggs::bucket::sub_values::BucketSubs;

/// The engine's node for a `date_histogram` of the date (or long) field
/// `field`.
pub fn node(field: &str, spec: &DateHistogramSpec) -> AggNode {
    AggNode::DateHistogram {
        // ALLOC: one field name an aggregation, bounded by the request's.
        field: field.to_owned(),
        missing: None,
        rounding: spec.rounding,
        hard_bounds: spec.hard_bounds,
        // ALLOC: empty; sub-aggregations are refused.
        subs: Vec::new(),
    }
}

/// A shard's result with no bucket: an unmapped field, or no match.
pub fn empty(spec: &DateHistogramSpec) -> DateHistogramResult {
    from_engine(spec, &[])
}

/// The result of the engine's first owning bucket, whose buckets carry their
/// rounded keys by key ascending.
pub fn from_engine(spec: &DateHistogramSpec, buckets: &[(i64, u64)]) -> DateHistogramResult {
    DateHistogramResult {
        spec: *spec,
        // ALLOC: one entry a bucket, at most the bucket limit (the engine
        // counts every key against it).
        buckets: buckets.to_vec(),
        subs: BucketSubs::default(),
    }
}

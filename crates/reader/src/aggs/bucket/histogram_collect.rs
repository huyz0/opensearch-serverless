//! Running a `histogram` on one shard: the engine's `histogram` node
//! (`NumericHistogramAggregator`) and its result as `InternalHistogram`
//! holds it.

use lucene_aggs::bucket_aggs::AggNode;
use lucene_aggs::metric_aggs::ValueKind;

use crate::aggs::bucket::histogram::{HistogramResult, HistogramSpec};
use crate::aggs::bucket::sub_values::BucketSubs;
use crate::failure::Failure;
use crate::search::ShardError;

/// The engine's node for a `histogram` of the long (or date) field `field`.
pub fn node(field: &str, spec: &HistogramSpec) -> AggNode {
    AggNode::Histogram {
        // ALLOC: one field name an aggregation, bounded by the request's.
        field: field.to_owned(),
        missing: None,
        kind: ValueKind::Long,
        interval: spec.interval,
        offset: spec.offset,
        hard_bounds: spec.hard_bounds,
        // ALLOC: empty; `keyed_subs` adds the sub-aggregations.
        subs: Vec::new(),
    }
}

/// A shard's result with no bucket: an unmapped field, or no match.
pub fn empty(spec: &HistogramSpec) -> HistogramResult {
    from_engine(spec, &[])
}

/// The result of the engine's first owning bucket, whose buckets carry
/// `floor((v - offset) / interval)`: each is keyed `n * interval + offset`.
pub fn from_engine(spec: &HistogramSpec, buckets: &[(f64, u64)]) -> HistogramResult {
    HistogramResult {
        // ALLOC: one spec a `histogram`, bounded by the request's.
        spec: spec.clone(),
        buckets: buckets
            .iter()
            .map(|&(n, count)| (n * spec.interval + spec.offset, count))
            // ALLOC: one entry a bucket, at most the bucket limit (the engine
            // counts every key against it).
            .collect(),
        subs: BucketSubs::default(),
    }
}

/// `AbstractHistogramAggregator`'s refusal of a missing or zero interval.
pub fn interval_refusal(spec: &HistogramSpec) -> Option<ShardError> {
    if spec.interval > 0.0 {
        return None;
    }
    Some(ShardError {
        status: 400,
        failure: Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!("interval must be positive, got: {:?}", spec.interval),
        ),
    })
}

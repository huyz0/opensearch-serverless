//! Running a `terms` on one shard: the engine's numeric `terms` node
//! (`NumericTermsAggregator`) and its result as `LongTerms` holds it.

use lucene_aggs::bucket_aggs::AggNode;
use lucene_aggs::metric_aggs::ValueKind;

use crate::aggs::bucket::sub_values::BucketSubs;
use crate::aggs::bucket::terms::{TermsResult, TermsSpec};
use crate::failure::Failure;
use crate::search::ShardError;

/// The engine's node for a `terms` of the long (or date) field `field`.
pub fn node(field: &str, spec: &TermsSpec) -> AggNode {
    AggNode::NumericTerms {
        // ALLOC: one field name an aggregation, bounded by the request's.
        field: field.to_owned(),
        missing: None,
        script: None,
        kind: ValueKind::Long,
        shard_size: spec.shard_size,
        order: spec.order,
        // ALLOC: empty; `keyed_subs` adds the sub-aggregations.
        subs: Vec::new(),
    }
}

/// A shard's result with no bucket: an unmapped field, or no match.
pub fn empty(spec: &TermsSpec) -> TermsResult {
    // ALLOC: empty.
    from_engine(spec, 0, Vec::new())
}

/// The result of the engine's first owning bucket: the documents of the
/// buckets it did not keep and the kept ones by key. A shard has no
/// slice-level error, so its `doc_count_error` is 0.
pub fn from_engine(spec: &TermsSpec, other: u64, buckets: Vec<(i64, u64)>) -> TermsResult {
    TermsResult {
        // ALLOC: one spec a `terms`, bounded by the request's.
        spec: spec.clone(),
        other_doc_count: other,
        doc_count_error: 0,
        buckets,
        subs: BucketSubs::default(),
    }
}

/// `TooManyBucketsException`, at 503 as OpenSearch answers it.
pub fn too_many_buckets(made: usize, limit: usize) -> ShardError {
    ShardError {
        status: 503,
        failure: Failure::plain(
            "too_many_buckets_exception",
            // ALLOC: error path; one message.
            format!(
                "Trying to create too many buckets. Must be less than or equal to: [{limit}] but \
                 was [{made}]. This limit can be set by changing the [search.max_buckets] cluster \
                 level setting."
            ),
        )
        .with_meta(
            "max_buckets",
            crate::failure::Meta::Int(u64::try_from(limit).unwrap_or(u64::MAX)),
        ),
    }
}

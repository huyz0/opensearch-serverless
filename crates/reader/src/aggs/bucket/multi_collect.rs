//! Running a `multi_terms` on one shard: the engine's `composite` node counts
//! every combination of the terms' values (`composite_collect`), and the
//! shard keeps the best `shard_size` of them by the order, as
//! `MultiTermsAggregator` does.

use lucene_aggs::bucket_aggs::AggResult;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::bucket::composite::KeyPart;
use crate::aggs::bucket::composite_collect;
use crate::aggs::bucket::multi::{MultiKey, MultiPart, MultiResult};
use crate::aggs::bucket::terms::TermsSpec;
use crate::aggs::bucket::terms_collect::too_many_buckets;
use crate::aggs::metric::MetricAgg;
use crate::aggs::partial::Value;
use crate::failure::Failure;
use crate::mapping::FieldType;
use crate::search::ShardError;

fn spec_of(agg: &MetricAgg) -> TermsSpec {
    agg.terms.as_deref().cloned().unwrap_or_default()
}

fn engine_error(reason: &str) -> ShardError {
    ShardError {
        status: 500,
        // ALLOC: error path; one message.
        failure: Failure::opensearch("exception", reason.to_owned()),
    }
}

/// The refusal of a term the node does not serve yet: a date, whose key
/// the response writes as text in its format.
pub fn check<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
) -> Result<(), ShardError> {
    let Some(composite) = agg.composite.as_deref() else {
        return Ok(());
    };
    for source in &composite.sources {
        let date = shard
            .meta
            .mapping
            .get(&source.field)
            .is_some_and(|f| f.ty == FieldType::Date);
        if date {
            return Err(ShardError {
                status: 400,
                failure: Failure::plain(
                    "illegal_argument_exception",
                    // ALLOC: error path; one message.
                    format!(
                        "aggregating date [{}] is not supported by this node yet",
                        source.field
                    ),
                ),
            });
        }
    }
    Ok(())
}

/// The result of no bucket: a term on an unmapped field, or no match.
pub fn empty(agg: &MetricAgg) -> Value {
    // ALLOC: one box a `multi_terms`, bounded by the request's.
    Value::Multi(Box::new(MultiResult::of_shard(&spec_of(agg), Vec::new())))
}

/// The shard-level result of `agg` from the engine's `composite` `result`.
pub fn value<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    result: Option<AggResult>,
) -> Result<Value, ShardError> {
    let Value::Composite(composite) = composite_collect::value_from(agg, shard, result)? else {
        return Err(engine_error("an aggregation result of another kind"));
    };
    if composite.buckets.len() > BUCKET_LIMIT {
        return Err(too_many_buckets(composite.buckets.len(), BUCKET_LIMIT));
    }
    // ALLOC: one entry a combination of the terms' values, at most the bucket limit.
    let mut counted = Vec::with_capacity(composite.buckets.len());
    for (parts, count) in composite.buckets {
        // ALLOC: one key a combination, as above.
        let key: Result<MultiKey, ShardError> = parts
            .into_iter()
            .map(|part| match part {
                KeyPart::Term(term) => {
                    // ALLOC: one term a part of a key, as above.
                    Ok(MultiPart::Text(String::from_utf8_lossy(&term).into_owned()))
                }
                KeyPart::Long(n) => Ok(MultiPart::Long(n)),
                KeyPart::Missing | KeyPart::Double(_) => {
                    Err(engine_error("a multi_terms key of another kind"))
                }
            })
            // ALLOC: as above.
            .collect();
        counted.push((key?, count));
    }
    // ALLOC: one box a `multi_terms`, bounded by the request's.
    Ok(Value::Multi(Box::new(MultiResult::of_shard(
        &spec_of(agg),
        counted,
    ))))
}

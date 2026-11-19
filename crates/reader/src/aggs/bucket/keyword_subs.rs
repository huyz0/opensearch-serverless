//! The sub-aggregations of a keyword `terms` on one shard: their engine
//! nodes under the `terms` node, and their per-bucket values from the
//! engine's result.

use lucene_aggs::bucket_aggs::{AggNode, AggResult};
use lucene_aggs::metric_aggs::MetricState;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::keyword_collect;
use crate::aggs::bucket::terms::TermsSpec;
use crate::aggs::metric::collect::{Resolved, node as metric_node, resolve, value_of};
use crate::aggs::metric::{DEFAULT_PRECISION, MetricAgg};
use crate::aggs::partial::{Format, Value};
use crate::failure::Failure;
use crate::search::ShardError;

/// Resolves each sub-aggregation of `agg` as a top-level one is resolved, so
/// an unreadable field fails the shard.
fn resolved<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
) -> Result<Vec<Resolved>, ShardError> {
    // ALLOC: one entry a sub-aggregation, bounded by the request's.
    agg.subs.iter().map(|sub| resolve(sub, shard)).collect()
}

/// `terms` as the engine runs it with `agg`'s sub-aggregations beneath: an
/// unmapped field's has no node, and its buckets read the empty metric.
pub fn node<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    spec: &TermsSpec,
    accepted: Option<lucene_aggs::bucket_aggs::AcceptedOrds>,
) -> Result<AggNode, ShardError> {
    let mut node = keyword_collect::node(&agg.field, spec, accepted);
    if let AggNode::Terms { subs, .. } = &mut node {
        for (sub, r) in agg.subs.iter().zip(resolved(agg, shard)?) {
            if matches!(r, Resolved::Numeric(_)) {
                subs.push(metric_node(sub, DEFAULT_PRECISION));
            }
        }
    }
    Ok(node)
}

fn engine_error(reason: &str) -> ShardError {
    ShardError {
        status: 500,
        // ALLOC: error path; one message.
        failure: Failure::opensearch("exception", reason.to_owned()),
    }
}

/// The shard-level result of the keyword `terms` `agg` from the engine's
/// `result`, with each kept bucket's sub-aggregation values.
pub fn value<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    spec: &TermsSpec,
    result: Option<AggResult>,
) -> Result<Value, ShardError> {
    if agg.subs.is_empty() {
        return keyword_collect::value(spec, result);
    }
    let Some(AggResult::Terms { buckets, subs }) = result else {
        return Err(engine_error("an aggregation result of another kind"));
    };
    let (other, kept) = buckets.into_iter().next().unwrap_or_default();
    let mut terms = keyword_collect::from_engine(spec, other, kept);
    let count = terms.buckets.len();
    let mut engine = subs.into_iter();
    // ALLOC: one list a kept bucket, at most the shard size.
    terms.subs = vec![Vec::new(); count];
    for (sub, r) in agg.subs.iter().zip(resolved(agg, shard)?) {
        let (format, states) = match r {
            Resolved::Numeric(format) => match engine.next() {
                Some(AggResult::Metric(states)) => (format, states),
                _ => return Err(engine_error("a sub-aggregation result of another kind")),
            },
            // The empty metric, in every bucket.
            // ALLOC: empty.
            _ => (Format::Raw, Vec::new()),
        };
        // ALLOC: one entry a sub-aggregation, bounded by the request's.
        terms.sub_meta.push((sub.name.clone(), format));
        let empty = MetricState::default();
        for (at, list) in terms.subs.iter_mut().enumerate() {
            // ALLOC: one value a sub-aggregation a kept bucket.
            list.push(value_of(sub, states.get(at).unwrap_or(&empty)));
        }
    }
    // ALLOC: one box a `terms`, bounded by the request's.
    Ok(Value::KeywordTerms(Box::new(terms)))
}

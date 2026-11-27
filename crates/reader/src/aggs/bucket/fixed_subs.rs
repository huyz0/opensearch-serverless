//! The sub-aggregations of a `range`, `filters` or `filter` on one shard:
//! their engine nodes under the bucket aggregation's node, each bucket's
//! values from the engine's fixed buckets, and the memory they hold. A
//! sub-aggregation is a single-value or stats metric, or a keyword `terms`.

use std::collections::HashMap;

use lucene_aggs::bucket_aggs::{AggNode, AggResult};
use lucene_search::global_ords::GlobalOrds;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::filters_collect::Queries;
use crate::aggs::bucket::keyword_collect::{Parent, SUB_COPIES};
use crate::aggs::bucket::{filters_collect, keyed_subs, keyword_subs, nested_subs, range_collect};
use crate::aggs::metric::{Metric, MetricAgg};
use crate::aggs::partial::Value;
use crate::search::ShardError;

/// Whether `agg` counts documents in a fixed list of buckets.
pub fn is_fixed(agg: &MetricAgg) -> bool {
    matches!(
        agg.metric,
        Metric::Range
            | Metric::Filters
            | Metric::Filter
            | Metric::Missing
            | Metric::Nested
            | Metric::ReverseNested
    )
}

/// Puts the nodes of `agg`'s sub-aggregations under `node`, the engine's
/// node for `agg`, building the ordinals of every keyword field they read
/// into `ords`.
pub fn attach<S: BlobStore + 'static>(
    node: &mut AggNode,
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    ords: &mut HashMap<String, std::sync::Arc<GlobalOrds>>,
    queries: &mut Queries<'_>,
) -> Result<(), ShardError> {
    if let AggNode::Range { subs, .. }
    | AggNode::Filters { subs, .. }
    | AggNode::Nested { subs, .. }
    | AggNode::ReverseNested { subs, .. } = node
    {
        *subs = keyword_subs::sub_nodes(agg, shard, ords, queries, false)?;
    }
    Ok(())
}

/// How many buckets `agg` makes.
pub(super) fn width(agg: &MetricAgg) -> usize {
    if agg.nested.is_some() {
        return 1;
    }
    match (agg.range.as_deref(), agg.filters.as_deref()) {
        (Some(range), _) => range.ranges.len(),
        (_, Some(filters)) => filters_collect::bucket_count(filters),
        _ => 0,
    }
}

/// The shard-level result of the `range`, `filters` or `filter` `agg` from
/// the engine's `result` (none for an unmapped field), with every bucket's
/// sub-aggregation values.
pub fn value<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    result: Option<AggResult>,
) -> Result<Value, ShardError> {
    let (docs, subs) = match result {
        Some(AggResult::Fixed { docs, subs, .. }) => (docs, subs),
        // ALLOC: empty.
        None => (Vec::new(), Vec::new()),
        Some(_) => {
            return Err(ShardError {
                status: 500,
                failure: crate::failure::Failure::opensearch(
                    "exception",
                    // ALLOC: error path; one message.
                    "an aggregation result of another kind".to_owned(),
                ),
            });
        }
    };
    let mut made = if filters_collect::counts_by_query(agg) {
        filters_collect::value(agg, &docs)
    } else {
        range_collect::value(agg, &docs)
    };
    if agg.subs.is_empty() {
        return Ok(made);
    }
    let (meta, lists) = keyword_subs::sub_values(agg, shard, subs, width(agg), false)?;
    match &mut made {
        Value::Range(r) => {
            r.sub_meta = meta;
            r.subs = lists;
        }
        Value::Filters(f) => {
            f.sub_meta = meta;
            f.subs = lists;
        }
        _ => {}
    }
    Ok(made)
}

/// The most bytes the sub-aggregations of the fixed buckets of every
/// aggregation in `aggs` hold on `shard`: see [`subs_bytes`].
pub fn reserved<S: BlobStore + 'static>(shard: &OpenShard<S>, aggs: &[MetricAgg]) -> usize {
    aggs.iter()
        .filter(|a| is_fixed(a) && !a.subs.is_empty())
        .fold(0usize, |total, agg| {
            total.saturating_add(subs_bytes(shard, agg, width(agg), 1))
        })
}

/// How many of `agg`'s buckets one document can land in: every range or
/// filter that holds it for a fixed kind, the bucket limit for a data-keyed
/// kind over a multi-valued field, else one.
fn own_fanout<S: BlobStore + 'static>(shard: &OpenShard<S>, agg: &MetricAgg) -> usize {
    if is_fixed(agg) {
        return width(agg).max(1);
    }
    let docs = usize::try_from(shard.reader.max_doc()).unwrap_or(usize::MAX);
    if keyed_subs::values(shard, &agg.field) > docs {
        crate::aggs::bucket::BUCKET_LIMIT
    } else {
        1
    }
}

/// The most bytes the sub-aggregations of `agg`'s `buckets` buckets hold on
/// `shard`: each bucket's engine states and values (the engine's, the
/// shard's and a reduce's), and a nested `terms`' own under the buckets.
/// `fanout` is how many of `agg`'s own parents' buckets one document can land
/// in (1 at the top level), which a nested level's buckets multiply by.
pub fn subs_bytes<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    agg: &MetricAgg,
    buckets: usize,
    fanout: usize,
) -> usize {
    let buckets = buckets.min(crate::aggs::bucket::BUCKET_LIMIT);
    let parent = Parent {
        cells: buckets,
        kept: buckets,
    };
    let per_value = std::mem::size_of::<lucene_aggs::metric_aggs::MetricState>()
        .saturating_add(SUB_COPIES.saturating_mul(std::mem::size_of::<Value>()));
    agg.subs.iter().fold(0usize, |total, sub| {
        // A numeric field holds numeric values; a keyword field none.
        let numeric_terms =
            sub.metric == Metric::Terms && keyed_subs::values(shard, &sub.field) > 0;
        total.saturating_add(if sub.metric == Metric::Terms && !numeric_terms {
            keyword_subs::reserved(shard, sub, parent)
        } else if nested_subs::is_nested(sub) || numeric_terms {
            nested_subs::reserved(
                shard,
                sub,
                buckets,
                fanout.saturating_mul(own_fanout(shard, agg)),
            )
        } else {
            buckets.saturating_mul(per_value)
        })
    })
}

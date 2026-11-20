//! The sub-aggregations of a `histogram` or `date_histogram` on one shard:
//! their engine nodes under the histogram's node, each bucket's values from
//! the engine's result, and the memory they hold. A sub-aggregation is a
//! single-value or stats metric, or a keyword `terms`.

use std::collections::HashMap;

use lucene_aggs::bucket_aggs::{AggNode, AggResult};
use lucene_aggs::metric_aggs::MetricState;
use lucene_search::global_ords::GlobalOrds;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::sub_values::BucketSubs;
use crate::aggs::bucket::{BUCKET_LIMIT, fixed_subs, keyword_subs};
use crate::aggs::metric::collect::{value_from, value_of};
use crate::aggs::metric::{Metric, MetricAgg};
use crate::aggs::partial::Value;
use crate::search::ShardError;

/// Whether `agg` is a `histogram` or a `date_histogram`.
pub fn is_keyed(agg: &MetricAgg) -> bool {
    matches!(agg.metric, Metric::Histogram | Metric::DateHistogram)
}

/// Puts the nodes of `agg`'s sub-aggregations under `node`, the engine's
/// node for `agg`, building the ordinals of every keyword field they read
/// into `ords`.
pub fn attach<S: BlobStore + 'static>(
    node: &mut AggNode,
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    ords: &mut HashMap<String, std::sync::Arc<GlobalOrds>>,
) -> Result<(), ShardError> {
    if let AggNode::Histogram { subs, .. } | AggNode::DateHistogram { subs, .. } = node {
        *subs = keyword_subs::sub_nodes(agg, shard, ords)?;
    }
    Ok(())
}

/// The shard-level result of the `histogram` or `date_histogram` `agg` from
/// the engine's `result` (none for an unmapped field), with every bucket's
/// sub-aggregation values.
pub fn value<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    result: Option<AggResult>,
) -> Result<Value, ShardError> {
    let (own, subs, count) = match result {
        Some(AggResult::Histogram { buckets, subs }) => {
            let count = buckets.first().map_or(0, Vec::len);
            (
                Some(AggResult::Histogram {
                    buckets,
                    // ALLOC: empty.
                    subs: Vec::new(),
                }),
                subs,
                count,
            )
        }
        Some(AggResult::DateHistogram { buckets, subs }) => {
            let count = buckets.first().map_or(0, Vec::len);
            (
                Some(AggResult::DateHistogram {
                    buckets,
                    // ALLOC: empty.
                    subs: Vec::new(),
                }),
                subs,
                count,
            )
        }
        // ALLOC: empty.
        other => (other, Vec::new(), 0),
    };
    let mut made = match own {
        Some(result) => value_from(agg, Some(result))?,
        None => value_of(agg, &MetricState::default()),
    };
    if agg.subs.is_empty() {
        return Ok(made);
    }
    let (meta, lists) = keyword_subs::sub_values(agg, shard, subs, count)?;
    let subs = BucketSubs {
        meta,
        empty: keyword_subs::empty_values(agg, shard)?,
        lists,
    };
    match &mut made {
        Value::Histogram(h) => h.subs = subs,
        Value::DateHistogram(h) => h.subs = subs,
        _ => {}
    }
    Ok(made)
}

/// How many values `field` holds across `shard`'s segments, from their
/// metadata: a document with several values lands in a bucket for each.
fn values<S: BlobStore + 'static>(shard: &OpenShard<S>, field: &str) -> usize {
    let mut total = 0usize;
    for segment in shard.reader.segment_readers() {
        let Some(info) = segment
            .field_infos()
            .fields
            .iter()
            .find(|i| i.name == field)
        else {
            continue;
        };
        let Some((meta, _)) = segment.doc_values_for_field(info.number) else {
            continue;
        };
        let entry = meta
            .numeric_entry(info.number)
            .or_else(|| meta.sorted_numeric_entry(info.number).map(|e| &e.numeric));
        if let Some(entry) = entry {
            total = total.saturating_add(usize::try_from(entry.num_values).unwrap_or(0));
        }
    }
    total
}

/// The most bytes the sub-aggregations of every `histogram` and
/// `date_histogram` in `aggs` hold on `shard`: a bucket holds a value of its
/// field at least, so there are at most as many buckets as the field's values
/// (the segments state how many), and no more than the bucket limit.
pub fn reserved<S: BlobStore + 'static>(shard: &OpenShard<S>, aggs: &[MetricAgg]) -> usize {
    aggs.iter()
        .filter(|a| is_keyed(a) && !a.subs.is_empty())
        .fold(0usize, |total, agg| {
            let buckets = values(shard, &agg.field).min(BUCKET_LIMIT);
            total.saturating_add(fixed_subs::subs_bytes(shard, agg, buckets))
        })
}

//! A request's aggregations with `global` among them: a `global` counts every
//! live document of the shard whatever the query (`GlobalAggregator`), so it
//! runs in a pass of its own under a `match_all` query, and the others in
//! the pass of the search's query; the results are put back in request order.

use std::collections::HashMap;

use lucene_search::BooleanQuery;
use lucene_search::field_norms::FieldNorms;
use lucene_search::multi_segment::OpenSegment;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::metric::collect::collect;
use crate::aggs::metric::{Metric, MetricAgg};
use crate::aggs::partial::Partial;
use crate::failure::Failure;
use crate::search::ShardError;

/// Whether `agg` or any aggregation under it reads the query's hits: a
/// `top_hits`, which a `global` does not run.
fn has_top_hits(agg: &MetricAgg) -> bool {
    agg.metric == Metric::TopHits || agg.subs.iter().any(has_top_hits)
}

/// [`collect`] over `aggs`, the `global` ones over `match_all` instead of
/// `query`. `filters` is one list a top-level aggregation, as `collect` takes it.
pub fn collect_with_global<S: BlobStore + 'static>(
    aggs: &[MetricAgg],
    shard: &OpenShard<S>,
    segments: &[OpenSegment<'_>],
    (query, match_all): (&BooleanQuery, &BooleanQuery),
    filters: &[Vec<BooleanQuery>],
    norms: &[Option<&HashMap<String, FieldNorms<'_>>>],
) -> Result<Vec<Partial>, ShardError> {
    if !aggs.iter().any(|a| a.metric == Metric::Global) {
        return collect(aggs, shard, segments, query, filters, norms);
    }
    if let Some(agg) = aggs
        .iter()
        .find(|a| a.metric == Metric::Global && a.subs.iter().any(has_top_hits))
    {
        return Err(ShardError {
            status: 400,
            failure: Failure::plain(
                "illegal_argument_exception",
                // ALLOC: error path; one message.
                format!(
                    "a [top_hits] sub-aggregation of the [global] [{}] is not supported by this \
                     node yet",
                    agg.name
                ),
            ),
        });
    }
    // ALLOC: one clone an aggregation, bounded by the request's.
    let (global, others): (Vec<usize>, Vec<usize>) =
        (0..aggs.len()).partition(|&i| aggs[i].metric == Metric::Global);
    let pick = |at: &[usize]| -> (Vec<MetricAgg>, Vec<Vec<BooleanQuery>>) {
        (
            // ALLOC: one clone an aggregation, bounded by the request's.
            at.iter().map(|&i| aggs[i].clone()).collect(),
            // ALLOC: one list an aggregation, bounded by the request's.
            at.iter()
                .map(|&i| filters.get(i).cloned().unwrap_or_default())
                .collect(),
        )
    };
    let (other_aggs, other_filters) = pick(&others);
    let (global_aggs, global_filters) = pick(&global);
    let mut other_parts =
        collect(&other_aggs, shard, segments, query, &other_filters, norms)?.into_iter();
    let mut global_parts = collect(
        &global_aggs,
        shard,
        segments,
        match_all,
        &global_filters,
        norms,
    )?
    .into_iter();
    // ALLOC: one result an aggregation, bounded by the request's.
    let mut parts = Vec::with_capacity(aggs.len());
    for agg in aggs {
        let next = if agg.metric == Metric::Global {
            global_parts.next()
        } else {
            other_parts.next()
        };
        parts.extend(next);
    }
    Ok(parts)
}

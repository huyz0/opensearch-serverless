//! A request's aggregations with `auto_date_histogram` among them: the shard
//! settles each one's rounding by running the engine's `date_histogram` (a
//! min and max pass, then a pass at the finest rounding the data fits, a
//! rounding up while the buckets pass what it holds), as
//! `AutoDateHistogramAggregator.FromSingle` rounds up while it collects; the
//! others run in the pass of the search's query.

use std::collections::HashMap;

use lucene_search::BooleanQuery;
use lucene_search::field_norms::FieldNorms;
use lucene_search::multi_segment::OpenSegment;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::auto_date::{AutoDateResult, AutoDateSpec};
use crate::aggs::bucket::date_histogram::DateHistogramSpec;
use crate::aggs::metric::collect::collect;
use crate::aggs::metric::collect_global::collect_with_global;
use crate::aggs::metric::{Metric, MetricAgg};
use crate::aggs::partial::{Partial, Value};
use crate::search::ShardError;

/// The rounding index the data fits: the finest where the range of the
/// rounded extremes is within the target of its largest interval.
fn start_index(spec: &AutoDateSpec, min: i64, max: i64) -> usize {
    let steps = spec.steps();
    let last = steps.len().saturating_sub(1);
    (0..steps.len())
        .find(|&i| {
            let rounding = spec.rounding(i);
            let range = rounding.round(max).saturating_sub(rounding.round(min));
            let allowed = i64::try_from(spec.target)
                .unwrap_or(i64::MAX)
                .saturating_mul(steps[i].max_rough_ms());
            range <= allowed
        })
        .unwrap_or(last)
}

/// One auto aggregation's shard result.
fn run_auto<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    segments: &[OpenSegment<'_>],
    query: &BooleanQuery,
    norms: &[Option<&HashMap<String, FieldNorms<'_>>>],
) -> Result<Partial, ShardError> {
    // ALLOC: one spec an aggregation, bounded by the request's.
    let spec = agg
        .auto_date
        .as_deref()
        .cloned()
        .unwrap_or(AutoDateSpec::FALLBACK);
    let probe = [
        MetricAgg::new("min", Metric::Min, &agg.field),
        MetricAgg::new("max", Metric::Max, &agg.field),
    ];
    let seen = collect(&probe, shard, segments, query, &[], norms)?;
    let format = seen
        .first()
        .map_or(crate::aggs::partial::Format::Raw, |p| p.format);
    let extremes = match (seen.first(), seen.get(1)) {
        (
            Some(Partial {
                value: Value::Min(lo),
                ..
            }),
            Some(Partial {
                value: Value::Max(hi),
                ..
            }),
        ) if lo.is_finite() && hi.is_finite() => Some((*lo as i64, *hi as i64)),
        _ => None,
    };
    // ALLOC: empty; a pass fills it with the buckets of one rounding.
    let (mut idx, mut buckets) = (0usize, Vec::new());
    if let Some((min, max)) = extremes {
        idx = start_index(&spec, min, max);
        loop {
            let mut histogram = MetricAgg::new(&agg.name, Metric::DateHistogram, &agg.field);
            // ALLOC: one spec a pass, bounded by the request's.
            histogram.date_histogram = Some(Box::new(DateHistogramSpec {
                rounding: spec.rounding(idx),
                min_doc_count: 1,
                ..DateHistogramSpec::default()
            }));
            let mut run = collect(&[histogram], shard, segments, query, &[], norms)?;
            buckets = match run.pop().map(|p| p.value) {
                Some(Value::DateHistogram(h)) => h.buckets,
                // ALLOC: empty.
                _ => Vec::new(),
            };
            if buckets.len() > spec.capacity(idx) && idx.saturating_add(1) < spec.steps().len() {
                idx = idx.saturating_add(1);
            } else {
                break;
            }
        }
    }
    Ok(Partial {
        // ALLOC: one name an aggregation, bounded by the request's.
        name: agg.name.clone(),
        format,
        // ALLOC: one box an `auto_date_histogram`, bounded by the request's.
        value: Value::AutoDateHistogram(Box::new(AutoDateResult {
            spec,
            idx,
            inner: 1,
            buckets,
            reduced: false,
        })),
    })
}

/// [`collect_with_global`] over the aggregations that are not an
/// `auto_date_histogram`, each of those run by its own passes; the results
/// are put back in request order.
pub fn collect_with_auto<S: BlobStore + 'static>(
    aggs: &[MetricAgg],
    shard: &OpenShard<S>,
    segments: &[OpenSegment<'_>],
    queries: (&BooleanQuery, &BooleanQuery),
    filters: &[Vec<BooleanQuery>],
    norms: &[Option<&HashMap<String, FieldNorms<'_>>>],
) -> Result<Vec<Partial>, ShardError> {
    if !aggs.iter().any(|a| a.metric == Metric::AutoDateHistogram) {
        return collect_with_global(aggs, shard, segments, queries, filters, norms);
    }
    let kept: Vec<usize> = (0..aggs.len())
        .filter(|&i| aggs[i].metric != Metric::AutoDateHistogram)
        // ALLOC: one index an aggregation, bounded by the request's.
        .collect();
    // ALLOC: one clone an aggregation, bounded by the request's.
    let others: Vec<MetricAgg> = kept.iter().map(|&i| aggs[i].clone()).collect();
    // ALLOC: one list an aggregation, bounded by the request's.
    let other_filters: Vec<Vec<BooleanQuery>> = kept
        .iter()
        .map(|&i| filters.get(i).cloned().unwrap_or_default())
        // ALLOC: as above.
        .collect();
    let mut other_parts =
        collect_with_global(&others, shard, segments, queries, &other_filters, norms)?.into_iter();
    // ALLOC: one result an aggregation, bounded by the request's.
    let mut parts = Vec::with_capacity(aggs.len());
    for agg in aggs {
        if agg.metric == Metric::AutoDateHistogram {
            parts.push(run_auto(agg, shard, segments, queries.0, norms)?);
        } else {
            parts.extend(other_parts.next());
        }
    }
    Ok(parts)
}

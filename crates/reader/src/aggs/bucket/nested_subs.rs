//! A bucket aggregation (`range`, `histogram`, `date_histogram`) as the
//! sub-aggregation of a bucket on one shard: the values of each owning
//! bucket from the engine's result for the sub-aggregation, which lists the
//! parent's buckets flattened in order ([ADR 0022]).
//!
//! [ADR 0022]: ../../../../../docs/adr/0022-nested-bucket-aggregations.md

use lucene_aggs::bucket_aggs::AggResult;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::keyword_collect::SUB_COPIES;
use crate::aggs::bucket::sub_values::{BucketSubs, SubValues};
use crate::aggs::bucket::{
    BUCKET_LIMIT, date_histogram_collect, date_histogram_spec, filters_collect, fixed_subs,
    histogram_collect, histogram_spec, keyed_subs, keyword_subs, range_collect, terms_collect,
    terms_spec,
};
use crate::aggs::metric::{Metric, MetricAgg};
use crate::aggs::partial::Value;
use crate::failure::Failure;
use crate::search::ShardError;

/// Whether `agg` is a bucket aggregation this runs under a bucket.
pub fn is_nested(agg: &MetricAgg) -> bool {
    matches!(
        agg.metric,
        Metric::Range
            | Metric::Filters
            | Metric::Filter
            | Metric::Histogram
            | Metric::DateHistogram
    )
}

fn other_kind() -> ShardError {
    ShardError {
        status: 500,
        // ALLOC: error path; one message.
        failure: Failure::opensearch(
            "exception",
            "an aggregation result of another kind".to_owned(),
        ),
    }
}

/// The values of `agg`'s own sub-aggregations over `total` buckets, from the
/// engine's `subs`: their names and formats, and a list of values a bucket.
fn inner<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    subs: Vec<AggResult>,
    total: usize,
) -> Result<SubValues, ShardError> {
    if agg.subs.is_empty() {
        // ALLOC: empty.
        return Ok((Vec::new(), Vec::new()));
    }
    keyword_subs::sub_values(agg, shard, subs, total, false)
}

/// One value an owning bucket for the nested bucket aggregation `agg` from
/// the engine's `result` (none for an unmapped field), which holds `owners`
/// owning buckets.
pub fn build<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    result: Option<AggResult>,
    owners: usize,
) -> Result<Vec<Value>, ShardError> {
    match agg.metric {
        Metric::Range | Metric::Filters | Metric::Filter => fixed(agg, shard, result, owners),
        Metric::Histogram => histogram(agg, shard, result, owners),
        _ => date_histogram(agg, shard, result, owners),
    }
}

fn fixed<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    result: Option<AggResult>,
    owners: usize,
) -> Result<Vec<Value>, ShardError> {
    let spec = agg.range.as_deref().cloned().unwrap_or_default();
    let width = fixed_subs::width(agg);
    let (docs, subs) = match result {
        Some(AggResult::Fixed { docs, subs, .. }) => (docs, subs),
        // ALLOC: empty.
        None => (Vec::new(), Vec::new()),
        Some(_) => return Err(other_kind()),
    };
    let (meta, lists) = inner(agg, shard, subs, owners.saturating_mul(width))?;
    let mut lists = lists.into_iter();
    // ALLOC: one value an owning bucket, bounded by the buckets above.
    let mut out = Vec::with_capacity(owners);
    for owner in 0..owners {
        let from = owner.saturating_mul(width);
        let mine = docs.get(from..from.saturating_add(width)).unwrap_or(&[]);
        let mut made = if agg.filters.is_some() {
            filters_collect::value(agg, mine)
        } else {
            // ALLOC: one box an owning bucket, bounded by the buckets above.
            Value::Range(Box::new(range_collect::from_counts(&spec, mine)))
        };
        if !agg.subs.is_empty() {
            // ALLOC: one list a bucket, bounded by the request's.
            let held: Vec<Vec<Value>> = lists.by_ref().take(width).collect();
            match &mut made {
                Value::Range(r) => {
                    // ALLOC: one entry a sub-aggregation, bounded by the request's.
                    r.sub_meta.clone_from(&meta);
                    r.subs = held;
                }
                Value::Filters(f) => {
                    // ALLOC: as above.
                    f.sub_meta.clone_from(&meta);
                    f.subs = held;
                }
                _ => {}
            }
        }
        out.push(made);
    }
    Ok(out)
}

fn histogram<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    result: Option<AggResult>,
    owners: usize,
) -> Result<Vec<Value>, ShardError> {
    let spec = histogram_spec(agg);
    let (mut buckets, subs) = match result {
        Some(AggResult::Histogram { buckets, subs }) => (buckets, subs),
        // ALLOC: empty.
        None => (Vec::new(), Vec::new()),
        Some(_) => return Err(other_kind()),
    };
    buckets.resize_with(owners, Vec::new);
    let total = buckets.iter().map(Vec::len).sum();
    let (meta, lists) = inner(agg, shard, subs, total)?;
    let empty = keyword_subs::empty_values(agg, shard)?;
    let mut lists = lists.into_iter();
    // ALLOC: one value an owning bucket, bounded by the buckets above.
    let mut out = Vec::with_capacity(owners);
    for mine in &buckets {
        let mut made = histogram_collect::from_engine(&spec, mine);
        if !agg.subs.is_empty() {
            made.subs = BucketSubs {
                // ALLOC: one entry a sub-aggregation, bounded by the request's.
                meta: meta.clone(),
                // ALLOC: as above.
                empty: empty.clone(),
                // ALLOC: one list a kept bucket, bounded by the key cap.
                lists: lists.by_ref().take(mine.len()).collect(),
            };
        }
        // ALLOC: one box an owning bucket, as above.
        out.push(Value::Histogram(Box::new(made)));
    }
    Ok(out)
}

fn date_histogram<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    result: Option<AggResult>,
    owners: usize,
) -> Result<Vec<Value>, ShardError> {
    let spec = date_histogram_spec(agg);
    let (mut buckets, subs) = match result {
        Some(AggResult::DateHistogram { buckets, subs }) => (buckets, subs),
        // ALLOC: empty.
        None => (Vec::new(), Vec::new()),
        Some(_) => return Err(other_kind()),
    };
    buckets.resize_with(owners, Vec::new);
    let total = buckets.iter().map(Vec::len).sum();
    let (meta, lists) = inner(agg, shard, subs, total)?;
    let empty = keyword_subs::empty_values(agg, shard)?;
    let mut lists = lists.into_iter();
    // ALLOC: one value an owning bucket, bounded by the buckets above.
    let mut out = Vec::with_capacity(owners);
    for mine in &buckets {
        let mut made = date_histogram_collect::from_engine(&spec, mine);
        if !agg.subs.is_empty() {
            made.subs = BucketSubs {
                // ALLOC: one entry a sub-aggregation, bounded by the request's.
                meta: meta.clone(),
                // ALLOC: as above.
                empty: empty.clone(),
                // ALLOC: one list a kept bucket, bounded by the key cap.
                lists: lists.by_ref().take(mine.len()).collect(),
            };
        }
        // ALLOC: one box an owning bucket, as above.
        out.push(Value::DateHistogram(Box::new(made)));
    }
    Ok(out)
}

/// The most bytes the nested bucket aggregation `agg` and everything under
/// it hold on `shard` when `parents` buckets own it and a document can land
/// in `fanout` of them: its own buckets (a bucket a range in each parent
/// bucket for a `range`; for a data-keyed kind a value of its field at least
/// for each of the `fanout` parent buckets the document is in; never past the
/// bucket limit, which the engine's key cap enforces), each with the states
/// and values of its sub-aggregations, and theirs beneath.
pub fn reserved<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    agg: &MetricAgg,
    parents: usize,
    fanout: usize,
) -> usize {
    let own = match agg.metric {
        Metric::Range | Metric::Filters | Metric::Filter => {
            fixed_subs::width(agg).saturating_mul(parents)
        }
        _ => keyed_subs::values(shard, &agg.field).saturating_mul(fanout),
    }
    .min(BUCKET_LIMIT);
    let per_bucket = std::mem::size_of::<Value>().saturating_mul(SUB_COPIES);
    own.saturating_mul(per_bucket)
        .saturating_add(fixed_subs::subs_bytes(shard, agg, own, fanout))
}

/// One value an owning bucket for the numeric `terms` `agg` from the
/// engine's `result` (none for an unmapped field), which holds `owners`
/// owning buckets: each owner's kept buckets, with their own
/// sub-aggregations beneath.
pub fn numeric_terms<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    result: Option<AggResult>,
    owners: usize,
) -> Result<Vec<Value>, ShardError> {
    let spec = terms_spec(agg);
    let (mut buckets, subs) = match result {
        Some(AggResult::NumericTerms { buckets, subs }) => (buckets, subs),
        // ALLOC: empty.
        None => (Vec::new(), Vec::new()),
        Some(_) => return Err(other_kind()),
    };
    buckets.resize_with(owners, Default::default);
    let total = buckets.iter().map(|b| b.1.len()).sum();
    let (meta, lists) = inner(agg, shard, subs, total)?;
    let empty = keyword_subs::empty_values(agg, shard)?;
    let mut lists = lists.into_iter();
    // ALLOC: one value an owning bucket, bounded by the buckets above.
    let mut out = Vec::with_capacity(owners);
    for (other, kept) in buckets {
        let kept_buckets = kept.len();
        let mut made = terms_collect::from_engine(&spec, other, kept);
        if !agg.subs.is_empty() {
            made.subs = BucketSubs {
                // ALLOC: one entry a sub-aggregation, bounded by the request's.
                meta: meta.clone(),
                // ALLOC: as above.
                empty: empty.clone(),
                // ALLOC: one list a kept bucket, bounded by the key cap.
                lists: lists.by_ref().take(kept_buckets).collect(),
            };
        }
        made.apply_shard_min();
        // ALLOC: one box an owning bucket, as above.
        out.push(Value::Terms(Box::new(made)));
    }
    Ok(out)
}

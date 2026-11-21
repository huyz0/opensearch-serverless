//! The `date_histogram` aggregation's request and shard-level result
//! (`DateHistogramAggregationBuilder`, `InternalDateHistogram` at OpenSearch
//! 3.10.0), and the reduce of results across shards
//! (`InternalDateHistogram.reduce`).

pub use lucene_aggs::bucket_aggs::{DateRounding, DateUnit, RoundingKind};

use crate::aggs::bucket::count_order::sort_by_count;
use crate::aggs::bucket::histogram::HistogramOrder;
use crate::aggs::bucket::running;
use crate::aggs::bucket::sub_values::{BucketSubs, reduce_keyed};
use crate::aggs::partial::Value;

/// What a `date_histogram` aggregation asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateHistogramSpec {
    /// The rounding of a value to its bucket's key, in the zone and with the
    /// offset asked for.
    pub rounding: DateRounding,
    /// Written as an object keyed by the key's text rather than an array.
    pub keyed: bool,
    /// The fewest documents a bucket of the answer holds; 0 fills the gaps
    /// between buckets with empty ones.
    pub min_doc_count: u64,
    /// How the buckets are ordered.
    pub order: HistogramOrder,
    /// `extended_bounds`, each end rounded down without the offset (as
    /// `LongBounds.round` does): with `min_doc_count` 0, empty buckets are
    /// made out to these.
    pub extended_bounds: Option<(Option<i64>, Option<i64>)>,
    /// `hard_bounds`, rounded the same way: values outside are not counted.
    pub hard_bounds: (Option<i64>, Option<i64>),
}

impl Default for DateHistogramSpec {
    /// A daily rounding in UTC, buckets by key ascending: what a request
    /// the parser accepted always overrides.
    fn default() -> Self {
        Self {
            rounding: DateRounding {
                kind: RoundingKind::Unit(DateUnit::Day),
                zone_ms: 0,
                offset: 0,
            },
            keyed: false,
            min_doc_count: 0,
            order: HistogramOrder::KeyAsc,
            extended_bounds: None,
            hard_bounds: (None, None),
        }
    }
}

/// What one shard (or one reduce) holds for a `date_histogram`:
/// `InternalDateHistogram`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DateHistogramResult {
    /// What the aggregation asked for.
    pub spec: DateHistogramSpec,
    /// `(key in epoch milliseconds, doc count)` by key ascending from a
    /// shard, in `order` from a reduce.
    pub buckets: Vec<(i64, u64)>,
    /// The buckets' sub-aggregations.
    pub subs: BucketSubs,
}

/// `InternalDateHistogram.reduce` as a final reduce: `None` unless there is
/// a part, every part is a `date_histogram`, and the answer is within the
/// bucket limit.
pub fn reduce(parts: &[Value]) -> Option<Value> {
    // ALLOC: one reference a shard, bounded by the shards that answered.
    let results: Option<Vec<&DateHistogramResult>> = parts
        .iter()
        .map(|p| match p {
            Value::DateHistogram(h) => Some(h.as_ref()),
            _ => None,
        })
        // ALLOC: one reference a shard, as above.
        .collect();
    let results = results?;
    if results.is_empty() {
        return None;
    }
    let reduced = match reduce_results(&results, running::remaining()) {
        Ok(reduced) => reduced,
        Err(made) => {
            running::add(made);
            return None;
        }
    };
    if !running::add(reduced.buckets.len()) {
        return None;
    }
    // ALLOC: one box a `date_histogram`, bounded by the request's.
    Some(Value::DateHistogram(Box::new(reduced)))
}

/// `InternalDateHistogram.nextKey`: the key of the bucket after `key`, which
/// the rounding without its offset rounds past (`nextRoundingValue`);
/// `None` where it overflows.
fn next_key(spec: &DateHistogramSpec, key: i64) -> Option<i64> {
    let offset = spec.rounding.offset;
    let plain = DateRounding {
        offset: 0,
        ..spec.rounding
    };
    plain
        .bucket(key.checked_sub(offset)?)
        .and_then(|(_, next)| next.checked_add(offset))
}

/// `LongBounds.round`: `bound` rounded down by the rounding without its
/// offset.
pub fn round_bound(rounding: &DateRounding, bound: i64) -> i64 {
    DateRounding {
        offset: 0,
        ..*rounding
    }
    .round(bound)
}

/// `buckets` with the empty ones between their keys, and out to
/// `extended_bounds`, made (`addEmptyBuckets`); `Err` with the count reached
/// when the answer passes `limit` buckets.
fn fill(
    spec: &DateHistogramSpec,
    buckets: Vec<(i64, u64)>,
    limit: usize,
) -> Result<Vec<(i64, u64)>, usize> {
    let offset = spec.rounding.offset;
    let (min_bound, max_bound) = spec.extended_bounds.unwrap_or((None, None));
    // ALLOC: at most one entry a bucket of the answer, which passing `limit`
    // stops.
    let mut out: Vec<(i64, u64)> = Vec::with_capacity(buckets.len().min(limit.saturating_add(1)));
    let push = |out: &mut Vec<(i64, u64)>, bucket: (i64, u64)| {
        out.push(bucket);
        if out.len() > limit {
            Err(out.len())
        } else {
            Ok(())
        }
    };
    // Empty buckets from `from` while `keep(key)`, as the loops of
    // `addEmptyBuckets` make them.
    let run =
        |out: &mut Vec<(i64, u64)>, from: i64, keep: &dyn Fn(i64) -> bool| -> Result<(), usize> {
            let mut key = from;
            while keep(key) {
                push(out, (key, 0))?;
                match next_key(spec, key) {
                    Some(next) => key = next,
                    None => break,
                }
            }
            Ok(())
        };
    match buckets.first() {
        None => {
            if let (Some(min), Some(max)) = (min_bound, max_bound) {
                let max = max.saturating_add(offset);
                run(&mut out, min.saturating_add(offset), &|k| k <= max)?;
            }
            return Ok(out);
        }
        Some(&(first, _)) => {
            if let Some(min) = min_bound {
                run(&mut out, min.saturating_add(offset), &|k| k < first)?;
            }
        }
    }
    let mut last: Option<i64> = None;
    for &bucket in &buckets {
        if let Some(last) = last
            && let Some(start) = next_key(spec, last)
        {
            run(&mut out, start, &|k| k < bucket.0)?;
        }
        last = Some(bucket.0);
        push(&mut out, bucket)?;
    }
    if let (Some(last), Some(max)) = (last, max_bound) {
        let max = max.saturating_add(offset);
        if max > last
            && let Some(start) = next_key(spec, last)
        {
            run(&mut out, start, &|k| k <= max)?;
        }
    }
    Ok(out)
}

/// The final reduce of `parts`, which are not empty, of one aggregation
/// (`InternalDateHistogram.reduce`): equal keys add, the buckets under
/// `min_doc_count` go, with `min_doc_count` 0 the gaps are filled, and the
/// buckets are ordered. `Err` holds the bucket count reached when the answer
/// would pass `limit` (`search.max_buckets`), which OpenSearch fails with.
pub fn reduce_results(
    parts: &[&DateHistogramResult],
    limit: usize,
) -> Result<DateHistogramResult, usize> {
    let Some(first) = parts.first() else {
        return Err(0);
    };
    let spec = first.spec;
    let total: usize = parts.iter().map(|p| p.buckets.len()).sum();
    if total > limit.saturating_mul(parts.len().max(1)) {
        return Err(total);
    }
    // ALLOC: one entry a bucket of the shards' results, at most `limit` a shard.
    let mut all: Vec<(i64, u64)> = Vec::with_capacity(total);
    for part in parts {
        all.extend_from_slice(&part.buckets);
    }
    all.sort_by_key(|b| b.0);
    // ALLOC: one entry a distinct key, at most `all`'s.
    let mut merged: Vec<(i64, u64)> = Vec::with_capacity(all.len());
    for (key, count) in all {
        match merged.last_mut() {
            Some(last) if last.0 == key => last.1 = last.1.saturating_add(count),
            _ => merged.push((key, count)),
        }
    }
    merged.retain(|b| b.1 >= spec.min_doc_count);
    let mut buckets = if spec.min_doc_count == 0 {
        fill(&spec, merged, limit)?
    } else {
        merged
    };
    if buckets.len() > limit {
        return Err(buckets.len());
    }
    match spec.order {
        HistogramOrder::KeyAsc => {}
        HistogramOrder::KeyDesc => buckets.reverse(),
        HistogramOrder::CountAsc | HistogramOrder::CountDesc => {
            sort_by_count(&mut buckets, spec.order == HistogramOrder::CountAsc);
        }
    }
    let subs = {
        // ALLOC: one key a bucket, bounded by the bucket limit.
        let keys: Vec<i64> = buckets.iter().map(|b| b.0).collect();
        // ALLOC: one key list a shard, bounded by the shards that answered.
        let held: Vec<Vec<i64>> = parts
            .iter()
            .map(|p| p.buckets.iter().map(|b| b.0).collect())
            // ALLOC: as above.
            .collect();
        let pairs: Vec<(&[i64], &BucketSubs)> = held
            .iter()
            .zip(parts)
            .map(|(k, p)| (k.as_slice(), &p.subs))
            // ALLOC: as above.
            .collect();
        reduce_keyed(&pairs, &keys)
    };
    Ok(DateHistogramResult {
        spec,
        buckets,
        subs,
    })
}

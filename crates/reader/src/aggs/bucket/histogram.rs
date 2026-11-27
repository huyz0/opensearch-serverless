//! The `histogram` aggregation's request and shard-level result
//! (`HistogramAggregationBuilder`, `InternalHistogram` at OpenSearch 3.10.0),
//! and the reduce of results across shards (`InternalHistogram.reduce`).

use std::cmp::Ordering;

use crate::aggs::bucket::count_order::sort_by_count;
use crate::aggs::bucket::histogram_sub_order::by_sub;
use crate::aggs::bucket::running;
use crate::aggs::bucket::sub_order::SubOrder;
use crate::aggs::bucket::sub_values::{BucketSubs, reduce_keyed};
use crate::aggs::partial::Value;

/// How a `histogram`'s buckets are ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistogramOrder {
    /// `_key` ascending: the default.
    KeyAsc,
    /// `_key` descending.
    KeyDesc,
    /// `_count` ascending.
    CountAsc,
    /// `_count` descending.
    CountDesc,
}

/// What a `histogram` aggregation asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct HistogramSpec {
    /// The width of a bucket; zero when the request gave none, which the
    /// shard refuses.
    pub interval: f64,
    /// Where the buckets start: a key is `n * interval + offset`.
    pub offset: f64,
    /// Written as an object keyed by the key's text rather than an array.
    pub keyed: bool,
    /// The fewest documents a bucket of the answer holds; 0 fills the gaps
    /// between buckets with empty ones.
    pub min_doc_count: u64,
    /// How the buckets are ordered.
    pub order: HistogramOrder,
    /// An `order` by a sub-aggregation, which the reduce applies in place of
    /// `order`.
    pub sub_order: Option<SubOrder>,
    /// `extended_bounds`: with `min_doc_count` 0, empty buckets are made
    /// out to these (`DoubleBounds`; an end not given is `None`).
    pub extended_bounds: Option<(Option<f64>, Option<f64>)>,
    /// `hard_bounds`: buckets outside are not made.
    pub hard_bounds: (Option<f64>, Option<f64>),
}

impl Default for HistogramSpec {
    /// `HistogramAggregationBuilder`'s defaults: no interval, an offset of
    /// 0, buckets by key ascending, none held back.
    fn default() -> Self {
        Self {
            interval: 0.0,
            offset: 0.0,
            keyed: false,
            min_doc_count: 0,
            order: HistogramOrder::KeyAsc,
            sub_order: None,
            extended_bounds: None,
            hard_bounds: (None, None),
        }
    }
}

/// What one shard (or one reduce) holds for a `histogram`: `InternalHistogram`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistogramResult {
    /// What the aggregation asked for.
    pub spec: HistogramSpec,
    /// `(key, doc count)` by key ascending from a shard, in `order` from a
    /// reduce.
    pub buckets: Vec<(f64, u64)>,
    /// The buckets' sub-aggregations.
    pub subs: BucketSubs,
}

/// `Double.compare`: `-0.0` below `0.0`, every NaN equal and above all else.
pub fn java_compare(a: f64, b: f64) -> Ordering {
    if a < b {
        return Ordering::Less;
    }
    if a > b {
        return Ordering::Greater;
    }
    canonical_bits(a).cmp(&canonical_bits(b))
}

/// `Double.doubleToLongBits` read as a signed long: one NaN.
pub(crate) fn canonical_bits(d: f64) -> i64 {
    if d.is_nan() {
        0x7ff8_0000_0000_0000
    } else {
        // ARITH: none; a reinterpretation of the bits.
        #[allow(clippy::cast_possible_wrap)]
        let bits = d.to_bits() as i64;
        bits
    }
}

/// `InternalHistogram.reduce` as a final reduce: `None` unless there is a
/// part, every part is a `histogram`, and the answer is within the bucket
/// limit.
pub fn reduce(parts: &[Value]) -> Option<Value> {
    // ALLOC: one reference a shard, bounded by the shards that answered.
    let results: Option<Vec<&HistogramResult>> = parts
        .iter()
        .map(|p| match p {
            Value::Histogram(h) => Some(h.as_ref()),
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
    // ALLOC: one box a `histogram`, bounded by the request's.
    Some(Value::Histogram(Box::new(reduced)))
}

/// `InternalHistogram.nextKey` of `key`: the key of the bucket after it.
fn next_key(spec: &HistogramSpec, key: f64) -> f64 {
    round(spec, key + spec.interval + spec.interval / 2.0)
}

/// `InternalHistogram.round`: the key of the bucket holding `key`.
fn round(spec: &HistogramSpec, key: f64) -> f64 {
    ((key - spec.offset) / spec.interval).floor() * spec.interval + spec.offset
}

/// `bucket`'s share of `buckets` once the empty ones between its keys, and
/// out to `extended_bounds`, are made (`addEmptyBuckets`); `Err` with the
/// count reached when the answer passes `limit` buckets.
fn fill(
    spec: &HistogramSpec,
    buckets: Vec<(f64, u64)>,
    limit: usize,
) -> Result<Vec<(f64, u64)>, usize> {
    let (min_bound, max_bound) = match spec.extended_bounds {
        Some((min, max)) => (
            min.unwrap_or(f64::INFINITY),
            max.unwrap_or(f64::NEG_INFINITY),
        ),
        None => (f64::INFINITY, f64::NEG_INFINITY),
    };
    // ALLOC: at most one entry a bucket of the answer, which passing `limit`
    // stops.
    let mut out: Vec<(f64, u64)> = Vec::with_capacity(buckets.len().min(limit.saturating_add(1)));
    let push = |out: &mut Vec<(f64, u64)>, bucket: (f64, u64)| {
        out.push(bucket);
        if out.len() > limit {
            Err(out.len())
        } else {
            Ok(())
        }
    };
    let Some(&first) = buckets.first() else {
        let mut key = round(spec, min_bound);
        while key <= max_bound {
            push(&mut out, (key, 0))?;
            key = next_key(spec, key);
        }
        return Ok(out);
    };
    if min_bound.is_finite() {
        let mut key = round(spec, min_bound);
        while key < first.0 {
            push(&mut out, (key, 0))?;
            key = next_key(spec, key);
        }
    }
    let mut last: Option<f64> = None;
    for &bucket in &buckets {
        if let Some(last) = last {
            let mut key = next_key(spec, last);
            while key < bucket.0 {
                push(&mut out, (key, 0))?;
                key = next_key(spec, key);
            }
        }
        last = Some(bucket.0);
        push(&mut out, bucket)?;
    }
    if let Some(last) = last {
        let mut key = next_key(spec, last);
        while key <= max_bound {
            push(&mut out, (key, 0))?;
            key = next_key(spec, key);
        }
    }
    Ok(out)
}

/// The final reduce of `parts`, which are not empty, of one aggregation
/// (`InternalHistogram.reduce`): equal keys add, the buckets under
/// `min_doc_count` go, with `min_doc_count` 0 the gaps are filled, and the
/// buckets are ordered. `Err` holds the bucket count reached when the answer
/// would pass `limit` (`search.max_buckets`), which OpenSearch fails with.
pub fn reduce_results(parts: &[&HistogramResult], limit: usize) -> Result<HistogramResult, usize> {
    let spec = parts.first().map(|p| p.spec.clone()).unwrap_or_default();
    let total: usize = parts.iter().map(|p| p.buckets.len()).sum();
    if total > limit.saturating_mul(parts.len().max(1)) {
        return Err(total);
    }
    // ALLOC: one entry a bucket of the shards' results, at most `limit` a shard.
    let mut all: Vec<(f64, u64)> = Vec::with_capacity(total);
    for part in parts {
        all.extend_from_slice(&part.buckets);
    }
    all.sort_by(|a, b| java_compare(a.0, b.0));
    // ALLOC: one entry a distinct key, at most `all`'s.
    let mut merged: Vec<(f64, u64)> = Vec::with_capacity(all.len());
    for (key, count) in all {
        match merged.last_mut() {
            Some(last) if java_compare(last.0, key) == Ordering::Equal => {
                last.1 = last.1.saturating_add(count);
            }
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
        _ if spec.sub_order.is_some() => {}
        HistogramOrder::KeyAsc => {}
        HistogramOrder::KeyDesc => buckets.reverse(),
        HistogramOrder::CountAsc | HistogramOrder::CountDesc => {
            sort_by_count(&mut buckets, spec.order == HistogramOrder::CountAsc);
        }
    }
    let subs = {
        // ALLOC: one key a bucket, bounded by the bucket limit.
        let keys: Vec<i64> = buckets.iter().map(|b| canonical_bits(b.0)).collect();
        // ALLOC: one key list a shard, bounded by the shards that answered.
        let held: Vec<Vec<i64>> = parts
            .iter()
            .map(|p| p.buckets.iter().map(|b| canonical_bits(b.0)).collect())
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
    let (buckets, subs) = match &spec.sub_order {
        Some(order) => by_sub(order, buckets, subs),
        None => (buckets, subs),
    };
    Ok(HistogramResult {
        spec,
        buckets,
        subs,
    })
}

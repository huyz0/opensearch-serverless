//! `auto_date_histogram`'s request and results: the roundings the builder
//! lists (`AutoDateHistogramAggregationBuilder.buildRoundings`) and the
//! reduce of the shards' results (`InternalAutoDateHistogram.reduce`) at
//! OpenSearch 3.10.0.

use lucene_aggs::bucket_aggs::{DateRounding, DateUnit, RegionZone, RoundingKind};

use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::partial::Value;
use crate::date_pattern::DatePattern;

/// One rounding of the builder's list.
pub struct Step {
    /// The calendar unit.
    pub unit: DateUnit,
    /// `roughEstimateDurationMillis`.
    pub rough_ms: i64,
    /// The unit's letter in `interval`.
    pub letter: &'static str,
    /// `innerIntervals`.
    pub inner: &'static [i64],
}

impl Step {
    /// `getMaximumInnerInterval`.
    fn max_inner(&self) -> i64 {
        self.inner.last().copied().unwrap_or(1)
    }

    /// `getMaximumRoughEstimateDurationMillis`.
    pub fn max_rough_ms(&self) -> i64 {
        self.rough_ms.saturating_mul(self.max_inner())
    }
}

/// The roundings, finest first.
pub const STEPS: [Step; 6] = [
    Step {
        unit: DateUnit::Second,
        rough_ms: 1_000,
        letter: "s",
        inner: &[1, 5, 10, 30],
    },
    Step {
        unit: DateUnit::Minute,
        rough_ms: 60_000,
        letter: "m",
        inner: &[1, 5, 10, 30],
    },
    Step {
        unit: DateUnit::Hour,
        rough_ms: 3_600_000,
        letter: "h",
        inner: &[1, 3, 12],
    },
    Step {
        unit: DateUnit::Day,
        rough_ms: 86_400_000,
        letter: "d",
        inner: &[1, 7],
    },
    Step {
        unit: DateUnit::Month,
        rough_ms: 2_592_000_000,
        letter: "M",
        inner: &[1, 3],
    },
    Step {
        unit: DateUnit::Year,
        rough_ms: 31_536_000_000,
        letter: "y",
        inner: &[1, 5, 10, 20, 50, 100],
    },
];

/// What an `auto_date_histogram` asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoDateSpec {
    /// `buckets`: the number of buckets aimed for.
    pub target: usize,
    /// The index in [`STEPS`] of the finest rounding (`minimum_interval`).
    pub base: usize,
    /// The zone's offset from UTC in milliseconds; 0 for a region.
    pub zone_ms: i64,
    /// An IANA region zone.
    pub region: Option<RegionZone>,
    /// `format`: how a key is written as text.
    pub format: Option<DatePattern>,
}

impl AutoDateSpec {
    /// What a request that was parsed always overrides.
    pub const FALLBACK: AutoDateSpec = AutoDateSpec {
        target: 10,
        base: 0,
        zone_ms: 0,
        region: None,
        format: None,
    };

    /// The roundings in use, finest first.
    pub fn steps(&self) -> &'static [Step] {
        STEPS.get(self.base..).unwrap_or(&[])
    }

    /// The rounding at `idx` of [`Self::steps`].
    pub fn rounding(&self, idx: usize) -> DateRounding {
        let unit = self.steps().get(idx).map_or(DateUnit::Year, |s| s.unit);
        DateRounding {
            kind: RoundingKind::Unit(unit),
            zone_ms: self.zone_ms,
            region: self.region,
            offset: 0,
        }
    }

    fn round(&self, idx: usize, key: i64) -> i64 {
        self.rounding(idx).round(key)
    }

    /// `Rounding.nextRoundingValue`: the key after `key`'s bucket.
    fn next(&self, idx: usize, key: i64) -> Option<i64> {
        self.rounding(idx).bucket(key).map(|(_, next)| next)
    }

    /// `buckets` times the largest inner interval of `idx`.
    pub fn capacity(&self, idx: usize) -> usize {
        let inner = self.steps().get(idx).map_or(1, Step::max_inner);
        self.target
            .saturating_mul(usize::try_from(inner).unwrap_or(1))
    }
}

/// What one shard (or one reduce) holds: `InternalAutoDateHistogram`.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoDateResult {
    /// What the aggregation asked for.
    pub spec: AutoDateSpec,
    /// The index of the rounding the keys are at, in [`AutoDateSpec::steps`].
    pub idx: usize,
    /// The merged inner interval (1 from a shard).
    pub inner: i64,
    /// `(key in epoch milliseconds, doc count)` by key ascending.
    pub buckets: Vec<(i64, u64)>,
    /// Whether this is a final reduce already (a shard result is not).
    pub reduced: bool,
}

impl AutoDateResult {
    /// `interval`: the inner interval and the unit's letter.
    pub fn interval(&self) -> String {
        let letter = self.spec.steps().get(self.idx).map_or("y", |s| s.letter);
        // ALLOC: one short text an aggregation.
        format!("{}{letter}", self.inner)
    }
}

/// `getAppropriateRounding`: the index of the rounding that fits the keys
/// `min` to `max`, from `idx` up.
pub fn appropriate_rounding(spec: &AutoDateSpec, min: i64, max: i64, idx: usize) -> usize {
    let steps = spec.steps();
    let last = steps.len().saturating_sub(1);
    if idx >= last {
        return idx;
    }
    let mut current = idx;
    // A rough pass: the first rounding whose estimate of the buckets needed
    // is within the target of the one before.
    let duration = max.saturating_sub(min);
    for (i, step) in steps.iter().enumerate().skip(idx.saturating_add(1)) {
        let rough = duration.checked_div(step.rough_ms).unwrap_or(0);
        if usize::try_from(rough).unwrap_or(usize::MAX) < spec.capacity(i.saturating_sub(1)) {
            current = i.saturating_sub(1);
            break;
        } else if i == last {
            current = i;
            break;
        }
    }
    loop {
        // Counting stops once the count passes what the rounding holds, as
        // only that comparison is read.
        let allowed = spec.capacity(current);
        let mut required = 0usize;
        let mut key = min;
        while key < max && required <= allowed {
            required = required.saturating_add(1);
            match spec.next(current, key) {
                Some(next) => key = next,
                None => break,
            }
        }
        current = current.saturating_add(1);
        if !(required > allowed && current < steps.len()) {
            return current.saturating_sub(1);
        }
    }
}

/// `mergeBuckets`: consecutive buckets with the same key at `idx` become one.
fn merge(spec: &AutoDateSpec, buckets: &[(i64, u64)], idx: usize) -> Vec<(i64, u64)> {
    // ALLOC: one entry a distinct key, at most the buckets given.
    let mut out: Vec<(i64, u64)> = Vec::with_capacity(buckets.len());
    for &(key, count) in buckets {
        let rounded = spec.round(idx, key);
        match out.last_mut() {
            Some(last) if last.0 == rounded => last.1 = last.1.saturating_add(count),
            _ => out.push((rounded, count)),
        }
    }
    out
}

/// `mergeBucketsIfNeeded`: the buckets at the rounding that fits them.
fn merge_if_needed(
    spec: &AutoDateSpec,
    buckets: Vec<(i64, u64)>,
    idx: usize,
) -> (Vec<(i64, u64)>, usize) {
    let (Some(first), Some(last)) = (buckets.first(), buckets.last()) else {
        return (buckets, idx);
    };
    let fit = appropriate_rounding(spec, first.0, last.0, idx);
    (merge(spec, &buckets, fit), fit)
}

/// `addEmptyBuckets`: the buckets at the rounding that fits them, with the
/// empty ones between made.
fn add_empty(
    spec: &AutoDateSpec,
    buckets: Vec<(i64, u64)>,
    idx: usize,
) -> (Vec<(i64, u64)>, usize) {
    let (Some(first), Some(last)) = (buckets.first(), buckets.last()) else {
        return (buckets, idx);
    };
    let fit = appropriate_rounding(spec, first.0, last.0, idx);
    let merged = merge(spec, &buckets, fit);
    // ALLOC: one entry a bucket and a gap, which the rounding's fit bounds.
    let mut out: Vec<(i64, u64)> = Vec::with_capacity(merged.len());
    for (i, &bucket) in merged.iter().enumerate() {
        if let Some(&(previous, _)) = i.checked_sub(1).and_then(|p| out.get(p)) {
            let mut key = spec.next(fit, previous);
            while let Some(k) = key.filter(|&k| k < bucket.0) {
                out.push((k, 0));
                if out.len() > BUCKET_LIMIT {
                    break;
                }
                key = spec.next(fit, k);
            }
        }
        out.push(bucket);
    }
    (out, fit)
}

/// `maybeMergeConsecutiveBuckets`: more buckets than the target merge, a
/// run of the inner interval at a time, at the first interval that fits.
fn merge_consecutive(
    spec: &AutoDateSpec,
    buckets: Vec<(i64, u64)>,
    idx: usize,
) -> (Vec<(i64, u64)>, i64) {
    if buckets.len() > spec.target {
        let inner = spec.steps().get(idx).map_or(&[][..], |s| s.inner);
        for &interval in inner {
            let step = usize::try_from(interval).unwrap_or(1).max(1);
            let resulting = buckets.len().div_ceil(step);
            if resulting <= spec.target {
                // ALLOC: one entry a merged bucket, at most the buckets.
                let mut out: Vec<(i64, u64)> = Vec::with_capacity(resulting);
                for run in buckets.chunks(step) {
                    let key = spec.round(idx, run[0].0);
                    let count = run.iter().fold(0u64, |n, b| n.saturating_add(b.1));
                    out.push((key, count));
                }
                return (out, interval);
            }
        }
    }
    (buckets, 1)
}

/// The final reduce of `parts`, which are not empty
/// (`InternalAutoDateHistogram.reduce`).
pub fn reduce_results(parts: &[&AutoDateResult]) -> Option<AutoDateResult> {
    let first = parts.first()?;
    let spec = first.spec.clone();
    let idx = parts.iter().map(|p| p.idx).max().unwrap_or(0);
    let mut all: Vec<(i64, u64)> = parts
        .iter()
        .flat_map(|p| p.buckets.iter().copied())
        // ALLOC: one entry a bucket of the shards' results.
        .collect();
    all.sort_by_key(|b| b.0);
    // reduceBuckets: the coarsest rounding any shard used, then the fit.
    let merged = merge(&spec, &all, idx);
    let (buckets, idx) = merge_if_needed(&spec, merged, idx);
    let (buckets, idx) = add_empty(&spec, buckets, idx);
    let (buckets, idx) = merge_if_needed(&spec, buckets, idx);
    let (buckets, inner) = merge_consecutive(&spec, buckets, idx);
    Some(AutoDateResult {
        spec,
        idx,
        inner,
        buckets,
        reduced: true,
    })
}

/// The reduce of shard results as a [`Value`]; `None` unless there is a part
/// and all are `auto_date_histogram`s.
pub fn reduce(parts: &[Value]) -> Option<Value> {
    // ALLOC: one reference a shard, bounded by the shards that answered.
    let results: Option<Vec<&AutoDateResult>> = parts
        .iter()
        .map(|p| match p {
            Value::AutoDateHistogram(r) => Some(r.as_ref()),
            _ => None,
        })
        // ALLOC: as above.
        .collect();
    let reduced = reduce_results(&results?)?;
    if !crate::aggs::bucket::running::add(reduced.buckets.len()) {
        return None;
    }
    // ALLOC: one box an `auto_date_histogram`, bounded by the request's.
    Some(Value::AutoDateHistogram(Box::new(reduced)))
}

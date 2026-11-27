//! The reduce of metric aggregations' shard-level results across shards
//! ([`crate::aggs::partial::Value`] holds each). It is ported from each class's `reduce` at
//! OpenSearch 3.10.0 (server/src/main/java/org/opensearch/search/aggregations/metrics/):
//! sums are added with `CompensatedSum`, in shard order.

use lucene_search::java_math::{JAVA_NAN, java_max, java_min};

use crate::aggs::metric::hll::Hll;
use crate::aggs::metric::tdigest::Digest;
use crate::aggs::partial::Value;

/// `CompensatedSum(0, 0)`.
#[derive(Clone, Copy, Debug, Default)]
struct CompensatedSum {
    value: f64,
    delta: f64,
}

impl CompensatedSum {
    /// `CompensatedSum.add(double)`.
    fn add(&mut self, value: f64) {
        // "If the value is Inf or NaN, just add it to the running tally."
        if !value.is_finite() {
            let tally = value + self.value;
            // A NaN's bits are Java's `Double.NaN` whatever the platform made.
            self.value = if tally.is_nan() { JAVA_NAN } else { tally };
        }
        if self.value.is_finite() {
            let corrected = value + self.delta;
            let updated = self.value + corrected;
            self.delta = corrected - (updated - self.value);
            self.value = updated;
        }
    }
}

/// The reduce of the shard results `parts`, all of one aggregation: `None`
/// when there are none, they are not all the same kind, or two sketches'
/// precisions differ (which OpenSearch refuses).
pub(crate) fn reduce_scalar(parts: &[Value]) -> Option<Value> {
    let first = parts.first()?;
    let mut sum = CompensatedSum::default();
    let mut squares = CompensatedSum::default();
    let mut count = 0u64;
    let mut low = f64::INFINITY;
    let mut high = f64::NEG_INFINITY;
    for part in parts {
        match (first, part) {
            (Value::Min(_), Value::Min(m)) => low = java_min(low, *m),
            (Value::Max(_), Value::Max(m)) => high = java_max(high, *m),
            (Value::Sum(_), Value::Sum(s)) => sum.add(*s),
            (Value::Avg { .. }, Value::Avg { sum: s, count: c }) => {
                count = count.saturating_add(*c);
                sum.add(*s);
            }
            (
                Value::Stats { .. },
                Value::Stats {
                    count: c,
                    sum: s,
                    min,
                    max,
                },
            ) => {
                count = count.saturating_add(*c);
                low = java_min(low, *min);
                high = java_max(high, *max);
                sum.add(*s);
            }
            (
                Value::ExtendedStats { .. },
                Value::ExtendedStats {
                    count: c,
                    sum: s,
                    min,
                    max,
                    sum_of_squares,
                    ..
                },
            ) => {
                count = count.saturating_add(*c);
                low = java_min(low, *min);
                high = java_max(high, *max);
                sum.add(*s);
                squares.add(*sum_of_squares);
            }
            (Value::ValueCount(_), Value::ValueCount(c)) => count = count.saturating_add(*c),
            _ => return None,
        }
    }
    Some(match first {
        Value::Min(_) => Value::Min(low),
        Value::Max(_) => Value::Max(high),
        Value::Sum(_) => Value::Sum(sum.value),
        Value::Avg { .. } => Value::Avg {
            sum: sum.value,
            count,
        },
        Value::Stats { .. } => Value::Stats {
            count,
            sum: sum.value,
            min: low,
            max: high,
        },
        Value::ExtendedStats { sigma, .. } => Value::ExtendedStats {
            count,
            sum: sum.value,
            min: low,
            max: high,
            sum_of_squares: squares.value,
            sigma: *sigma,
        },
        Value::ValueCount(_) => Value::ValueCount(count),
        // Reduced above.
        _ => return None,
    })
}

/// `InternalCardinality.reduce`: every sketch merged, in shard order, into
/// an empty one of the first's precision; no sketch when none saw a value.
pub(crate) fn reduce_cardinality(parts: &[Value]) -> Option<Value> {
    let mut reduced: Option<Hll> = None;
    for part in parts {
        let Value::Cardinality(sketch) = part else {
            return None;
        };
        let Some(sketch) = sketch else { continue };
        if !reduced
            .get_or_insert_with(|| Hll::empty_like(sketch))
            .merge(sketch)
        {
            return None;
        }
    }
    Some(Value::Cardinality(reduced))
}

/// `AbstractInternalTDigestPercentiles.reduce`: every digest added, in
/// shard order, into an empty one of the first's compression, with the
/// first's keys; no digest when no shard had one. It holds the reduced
/// digest and one input's copy at a time: a caller reserves twice
/// `tdigest::digest_bytes` of the compression.
pub(crate) fn reduce_percentiles(parts: &[Value]) -> Option<Value> {
    let Some(Value::Percentiles {
        ranks,
        keys,
        keyed,
        compression,
        ..
    }) = parts.first()
    else {
        return None;
    };
    let mut reduced: Option<Digest> = None;
    for part in parts {
        let Value::Percentiles {
            ranks: r, digest, ..
        } = part
        else {
            return None;
        };
        if r != ranks {
            return None;
        }
        if let Some(d) = digest {
            reduced
                .get_or_insert_with(|| Digest::new(*compression))
                .add_digest(d);
        }
    }
    if let Some(d) = reduced.as_mut() {
        d.settle();
    }
    Some(Value::Percentiles {
        ranks: *ranks,
        // ALLOC: one key an asked-for percent, bounded by the request's.
        keys: keys.clone(),
        keyed: *keyed,
        compression: *compression,
        digest: reduced,
    })
}

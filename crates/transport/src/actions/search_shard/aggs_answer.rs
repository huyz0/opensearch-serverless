//! The reply side of a forwarded search's aggregations: each shard-level
//! result written as `InternalAggregations` writes it.

use super::aggs::{AggAnswer, AggKind, AggValue, write_format};
use crate::error::WireError;
use crate::stream::Output;

/// The writeable name of an answer: the aggregation's own, but for the
/// percentiles, which are named for their t-digest.
fn answer_name(kind: AggKind, value: &AggValue) -> &'static str {
    match value {
        AggValue::Terms(t) => return t.name(),
        AggValue::Range(r) => return r.name(),
        AggValue::Rare(r) => return r.name(),
        _ => {}
    }
    match kind {
        AggKind::Percentiles => "tdigest_percentiles",
        AggKind::PercentileRanks => "tdigest_percentile_ranks",
        other => other.name(),
    }
}

/// Writes `answers` as `InternalAggregations.writeTo` does: a list of named
/// writeables, each its type name and its `InternalAggregation.writeTo`.
///
/// # Errors
///
/// [`WireError::TooLarge`] past the writer's limit.
pub fn write_answers(answers: &[AggAnswer<'_>], out: &mut Output<'_>) -> Result<(), WireError> {
    out.count(answers.len())?;
    for a in answers {
        write_one(a.name, &a.value, a.date, out)?;
    }
    Ok(())
}

/// One result as a named writeable: its type name, its name, and its
/// `InternalAggregation.writeTo`.
pub(super) fn write_one(
    name: &str,
    value: &AggValue,
    date: bool,
    out: &mut Output<'_>,
) -> Result<(), WireError> {
    {
        let (kind, ()) = match value {
            AggValue::Min(_) => (AggKind::Min, ()),
            AggValue::Max(_) => (AggKind::Max, ()),
            AggValue::Sum(_) => (AggKind::Sum, ()),
            AggValue::Avg { .. } => (AggKind::Avg, ()),
            AggValue::ValueCount(_) => (AggKind::ValueCount, ()),
            AggValue::Stats { .. } => (AggKind::Stats, ()),
            AggValue::Cardinality(_) => (AggKind::Cardinality, ()),
            AggValue::Terms(_) => (AggKind::Terms, ()),
            AggValue::DateHistogram(_) => (AggKind::DateHistogram, ()),
            AggValue::Range(_) => (AggKind::Range, ()),
            AggValue::Rare(_) => (AggKind::RareTerms, ()),
            AggValue::Multi(_) => (AggKind::MultiTerms, ()),
            AggValue::Percentiles { ranks: false, .. } => (AggKind::Percentiles, ()),
            AggValue::Percentiles { ranks: true, .. } => (AggKind::PercentileRanks, ()),
        };
        out.string(answer_name(kind, value))?;
        out.string(name)?;
        out.byte(0xFF)?; // metadata: null
        match value {
            AggValue::Min(v) | AggValue::Max(v) | AggValue::Sum(v) => {
                write_format(out, date)?;
                out.bytes(&v.to_bits().to_be_bytes())?;
            }
            AggValue::Percentiles {
                keys,
                digest,
                keyed,
                ..
            } => {
                write_format(out, date)?;
                out.count(keys.len())?;
                for k in keys {
                    out.bytes(&k.to_bits().to_be_bytes())?;
                }
                out.count(digest.len())?; // `writeVInt(byteSize)`
                out.bytes(digest)?;
                out.bool(*keyed)?;
            }
            AggValue::Terms(t) => t.write(date, out)?,
            AggValue::DateHistogram(h) => h.write(out)?,
            AggValue::Range(r) => r.write(date, out)?,
            AggValue::Rare(r) => r.write(out)?,
            AggValue::Multi(m) => m.write(out)?,
            AggValue::Cardinality(sketch) => {
                write_format(out, date)?;
                out.bool(sketch.is_some())?;
                if let Some(bytes) = sketch {
                    out.bytes(bytes)?;
                }
            }
            AggValue::Avg { sum, count } => {
                write_format(out, date)?;
                out.bytes(&sum.to_bits().to_be_bytes())?;
                out.vlong(*count)?;
            }
            AggValue::ValueCount(n) => out.vlong(*n)?,
            AggValue::Stats {
                count,
                min,
                max,
                sum,
            } => {
                write_format(out, date)?;
                out.vlong(*count)?;
                for v in [min, max, sum] {
                    out.bytes(&v.to_bits().to_be_bytes())?;
                }
            }
        }
    }
    Ok(())
}

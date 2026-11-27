//! The aggregations of a forwarded search: `AggregatorFactories.Builder` as
//! the Java shell writes it, for the single-value metrics a Rust node runs
//! (`min`, `max`, `sum`, `avg`, `value_count`, `stats` over one field), and the
//! shard's `InternalAggregations` answering them.
//!
//! Anything past that (a sub-aggregation, a pipeline aggregation, metadata, a
//! script, a format, a `missing` value, a time zone, another type) is
//! refused by name.

use super::Refusal;
use super::aggs_histogram::{self, DateHistogramAnswer, DateHistogramRequest};
use super::aggs_multi::{self, MultiAnswer, MultiRequest};
use super::aggs_percentiles::{self, PercentilesRequest};
use super::aggs_range::{self, RangeAnswer, RangesRequest};
use super::aggs_rare::{self, RareAnswer, RareRequest};
use super::aggs_terms::{self, TermsAnswer, TermsRequest};
use crate::error::WireError;
use crate::stream::{Input, Output};

/// The most aggregations one request may carry.
pub const MAX_AGGREGATIONS: usize = 64;

pub use super::aggs_kind::AggKind;

/// One aggregation asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct AggRequest {
    /// The aggregation's name.
    pub name: String,
    /// What it computes.
    pub kind: AggKind,
    /// The field it reads.
    pub field: String,
    /// A `cardinality`'s `precision_threshold`.
    pub precision_threshold: Option<i64>,
    /// A `percentiles`' or `percentile_ranks`' keys and options.
    pub percentiles: Option<PercentilesRequest>,
    /// A `terms`' options.
    pub terms: Option<TermsRequest>,
    /// A `date_histogram`'s options.
    pub histogram: Option<DateHistogramRequest>,
    /// A `range`'s or `date_range`'s ranges.
    pub ranges: Option<RangesRequest>,
    /// A `rare_terms`' options.
    pub rare: Option<RareRequest>,
    /// A `multi_terms`' terms and options.
    pub multi: Option<MultiRequest>,
    /// The sub-aggregations of a bucket aggregation, in request order.
    pub subs: Vec<AggRequest>,
}

/// One shard-level result.
#[derive(Debug, Clone, PartialEq)]
pub enum AggValue {
    /// `InternalMin`'s value; positive infinity when nothing matched.
    Min(f64),
    /// `InternalMax`'s value; negative infinity when nothing matched.
    Max(f64),
    /// `InternalSum`'s value.
    Sum(f64),
    /// `InternalAvg`'s sum and count.
    Avg {
        /// The sum.
        sum: f64,
        /// How many values.
        count: i64,
    },
    /// `InternalValueCount`'s value.
    ValueCount(i64),
    /// `InternalCardinality`: the sketch as `AbstractHyperLogLogPlusPlus`
    /// writes it, or none when no value was seen.
    Cardinality(Option<Vec<u8>>),
    /// `StringTerms` or `LongTerms`.
    Terms(TermsAnswer),
    /// `InternalDateHistogram`.
    DateHistogram(DateHistogramAnswer),
    /// `InternalRange` or `InternalDateRange`.
    Range(RangeAnswer),
    /// `StringRareTerms` or `LongRareTerms`.
    Rare(RareAnswer),
    /// `InternalMultiTerms`.
    Multi(MultiAnswer),
    /// `InternalTDigestPercentiles` or `InternalTDigestPercentileRanks`.
    Percentiles {
        /// Ranks rather than percentiles.
        ranks: bool,
        /// The percents or the values ranked.
        keys: Vec<f64>,
        /// The digest as `TDigestState.write` writes it.
        digest: Vec<u8>,
        /// `keyed`.
        keyed: bool,
    },
    /// `InternalStats`.
    Stats {
        /// How many values.
        count: i64,
        /// The smallest; positive infinity for none.
        min: f64,
        /// The largest; negative infinity for none.
        max: f64,
        /// The sum.
        sum: f64,
    },
}

/// A result with the name of its aggregation.
#[derive(Debug, Clone, PartialEq)]
pub struct AggAnswer<'a> {
    /// The aggregation's name.
    pub name: &'a str,
    /// Its value.
    pub value: AggValue,
    /// Whether the field is a date, which the answer's format says
    /// (`DocValueFormat.DateTime` of the default date format) in place of
    /// `raw`.
    pub date: bool,
}

/// `out.writeNamedWriteable(format)` of `DocValueFormat.RAW`, or of the
/// `DocValueFormat.DateTime` a date field of the default format, in UTC and
/// in milliseconds, has.
pub(super) fn write_format(out: &mut Output<'_>, date: bool) -> Result<(), WireError> {
    if !date {
        return out.string("raw"); // writes nothing more
    }
    out.string("date_time")?;
    out.string("strict_date_time_no_millis||strict_date_optional_time||epoch_millis")?;
    out.optional_string(Some("strict_date_optional_time"))?;
    out.string("Z")?; // UTC
    out.vint(0) // `Resolution.MILLISECONDS`
}

/// A generic value that is null (`writeGenericValue(null)` is the byte -1).
fn null_generic(input: &mut Input<'_>, what: &'static str) -> Result<(), Refusal> {
    match input.byte(what)? {
        0xFF => Ok(()),
        _ => Err(Refusal::Unsupported(what)),
    }
}

fn none(input: &mut Input<'_>, what: &'static str) -> Result<(), Refusal> {
    if input.bool(what)? {
        return Err(Refusal::Unsupported(what));
    }
    Ok(())
}

/// An empty `AggregatorFactories.Builder`: no aggregation, no pipeline.
fn empty_builder(input: &mut Input<'_>, what: &'static str) -> Result<(), Refusal> {
    if input.vint(what)? != 0 || input.vint(what)? != 0 {
        return Err(Refusal::Unsupported(what));
    }
    Ok(())
}

/// How deep sub-aggregations may nest.
const MAX_DEPTH: usize = 3;

/// Reads the `AggregatorFactories.Builder` after its presence flag was read
/// as true.
///
/// # Errors
///
/// [`Refusal`] for malformed bytes or an aggregation this node does not run.
pub fn read(input: &mut Input<'_>) -> Result<Vec<AggRequest>, Refusal> {
    let mut budget = MAX_AGGREGATIONS;
    read_builder(input, 0, &mut budget)
}

/// One `AggregatorFactories.Builder`: the aggregations, then the pipeline
/// aggregations, of which there are none. `budget` is how many aggregations
/// the whole request may still hold.
fn read_builder(
    input: &mut Input<'_>,
    depth: usize,
    budget: &mut usize,
) -> Result<Vec<AggRequest>, Refusal> {
    let count = input.vint("aggregations")?;
    let count = usize::try_from(count)
        .ok()
        .filter(|n| *n <= *budget)
        .ok_or(Refusal::TooLarge("aggregations"))?;
    *budget = budget.saturating_sub(count);
    // ALLOC: one entry an aggregation, within the request's budget.
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(read_one(input, depth, budget)?);
    }
    // Pipeline aggregations.
    if input.vint("pipeline aggregations")? != 0 {
        return Err(Refusal::Unsupported("pipeline aggregations"));
    }
    Ok(out)
}

fn read_one(
    input: &mut Input<'_>,
    depth: usize,
    budget: &mut usize,
) -> Result<AggRequest, Refusal> {
    let kind_name = input.string("aggregation type")?;
    let Some(kind) = AggKind::from_name(&kind_name) else {
        return Err(Refusal::Unsupported("an aggregation of another type"));
    };
    let name = input.string("aggregation name")?;
    let subs = if depth >= MAX_DEPTH {
        empty_builder(input, "sub-aggregations nested so deep")?;
        // ALLOC: empty.
        Vec::new()
    } else {
        read_builder(input, depth.saturating_add(1), budget)?
    };
    if !subs.is_empty()
        && !matches!(
            kind,
            AggKind::Terms | AggKind::DateHistogram | AggKind::Range | AggKind::DateRange
        )
    {
        return Err(Refusal::Unsupported("sub-aggregations"));
    }
    null_generic(input, "aggregation metadata")?;
    if kind == AggKind::MultiTerms {
        let multi = aggs_multi::read(input)?;
        return Ok(AggRequest {
            name,
            kind,
            // ALLOC: one field name, bounded by the request's.
            field: multi.fields.first().cloned().unwrap_or_default(),
            precision_threshold: None,
            percentiles: None,
            terms: None,
            histogram: None,
            ranges: None,
            rare: None,
            multi: Some(multi),
            subs,
        });
    }
    if matches!(
        kind,
        AggKind::ValueCount | AggKind::Cardinality | AggKind::Terms | AggKind::RareTerms
    ) {
        // `serializeTargetValueType`: a null writeable.
        none(input, "a value type target")?;
    }
    let field = input
        .optional_string("aggregation field")?
        .ok_or(Refusal::Unsupported("an aggregation without a field"))?;
    none(input, "an aggregation script")?;
    none(input, "an aggregation value type")?;
    if input.optional_string("aggregation format")?.is_some() {
        return Err(Refusal::Unsupported("an aggregation format"));
    }
    null_generic(input, "an aggregation missing value")?;
    none(input, "an aggregation time zone")?;
    let mut precision_threshold = None;
    if kind == AggKind::Cardinality {
        if input.bool("a precision threshold flag")? {
            precision_threshold = Some(input.long("a precision threshold")?);
        }
        if input.optional_string("an execution hint")?.is_some() {
            return Err(Refusal::Unsupported("an execution hint"));
        }
    }
    let mut percentiles = None;
    if matches!(kind, AggKind::Percentiles | AggKind::PercentileRanks) {
        percentiles = Some(aggs_percentiles::read(input)?);
    }
    let terms = if kind == AggKind::Terms {
        Some(aggs_terms::read(input)?)
    } else {
        None
    };
    let histogram = if kind == AggKind::DateHistogram {
        Some(aggs_histogram::read(input)?)
    } else {
        None
    };
    let ranges = if matches!(kind, AggKind::Range | AggKind::DateRange) {
        Some(aggs_range::read(input)?)
    } else {
        None
    };
    let rare = if kind == AggKind::RareTerms {
        Some(aggs_rare::read(input)?)
    } else {
        None
    };
    Ok(AggRequest {
        name,
        kind,
        field,
        precision_threshold,
        percentiles,
        terms,
        histogram,
        ranges,
        rare,
        multi: None,
        subs,
    })
}

/// Writes `aggs` as `AggregatorFactories.Builder.writeTo` does.
///
/// # Errors
///
/// [`WireError::TooLarge`] past the writer's limit.
pub fn write(aggs: &[AggRequest], out: &mut Output<'_>) -> Result<(), WireError> {
    out.count(aggs.len())?;
    for agg in aggs {
        out.string(agg.kind.name())?;
        out.string(&agg.name)?;
        write(&agg.subs, out)?; // sub-aggregations and their pipelines
        out.byte(0xFF)?; // metadata: null
        if let Some(m) = &agg.multi {
            aggs_multi::write(m, out)?;
            continue;
        }
        if matches!(
            agg.kind,
            AggKind::ValueCount | AggKind::Cardinality | AggKind::Terms | AggKind::RareTerms
        ) {
            out.bool(false)?; // target value type
        }
        out.optional_string(Some(&agg.field))?;
        out.bool(false)?; // script
        out.bool(false)?; // value type
        out.optional_string(None)?; // format
        out.byte(0xFF)?; // missing: null
        out.bool(false)?; // time zone
        if agg.kind == AggKind::Cardinality {
            out.bool(agg.precision_threshold.is_some())?;
            if let Some(t) = agg.precision_threshold {
                out.long(t)?;
            }
            out.optional_string(None)?; // execution hint
        }
        if let Some(t) = &agg.terms {
            aggs_terms::write(t, out)?;
        }
        if let Some(h) = &agg.histogram {
            aggs_histogram::write(h, out)?;
        }
        if let Some(r) = &agg.ranges {
            aggs_range::write(r, out)?;
        }
        if let Some(r) = &agg.rare {
            aggs_rare::write(r, out)?;
        }
        if let Some(p) = &agg.percentiles {
            out.count(p.keys.len())?;
            for k in &p.keys {
                out.bytes(&k.to_bits().to_be_bytes())?;
            }
            out.bool(p.keyed)?;
            out.bool(p.compression.is_some())?;
            if let Some(c) = p.compression {
                out.byte(0)?; // the t-digest
                out.bytes(&c.to_bits().to_be_bytes())?;
            }
        }
    }
    out.vint(0) // pipeline aggregations
}

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

#[cfg(test)]
#[path = "aggs_tests.rs"]
mod tests;

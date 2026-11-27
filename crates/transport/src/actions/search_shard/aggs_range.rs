//! The `range` and `date_range` aggregations of a forwarded search:
//! `AbstractRangeBuilder`'s own fields as `innerWriteTo` writes them, and the
//! shard's `InternalRange` or `InternalDateRange` answering them, as
//! `InternalRange.doWriteTo` writes them.

use super::Refusal;
use super::aggs::write_format;
use super::aggs::write_one;
use super::aggs_terms::SubAnswer;
use crate::error::WireError;
use crate::stream::{Input, Output};

/// The most ranges one aggregation may carry.
const MAX_RANGES: usize = 128;

/// One range asked for (`RangeAggregator.Range`).
#[derive(Debug, Clone, PartialEq)]
pub struct RangeAsk {
    /// The `key`, when one was given.
    pub key: Option<String>,
    /// The lower bound; negative infinity for none.
    pub from: f64,
    /// The lower bound as text, when it was given as text.
    pub from_text: Option<String>,
    /// The upper bound; positive infinity for none.
    pub to: f64,
    /// The upper bound as text, when it was given as text.
    pub to_text: Option<String>,
}

/// What a `range` or `date_range` asks beyond its field.
#[derive(Debug, Clone, PartialEq)]
pub struct RangesRequest {
    /// The ranges, in the order written.
    pub ranges: Vec<RangeAsk>,
    /// `keyed`.
    pub keyed: bool,
}

/// Reads the fields after a range builder's values-source fields.
///
/// # Errors
///
/// [`Refusal`] for malformed bytes or more ranges than are served.
pub fn read(input: &mut Input<'_>) -> Result<RangesRequest, Refusal> {
    let count = usize::try_from(input.vint("ranges")?)
        .ok()
        .filter(|n| *n <= MAX_RANGES)
        .ok_or(Refusal::TooLarge("ranges"))?;
    // ALLOC: one range each, at most MAX_RANGES.
    let mut ranges = Vec::with_capacity(count);
    for _ in 0..count {
        let key = input.optional_string("a range key")?;
        let from_text = input.optional_string("a range lower bound text")?;
        let to_text = input.optional_string("a range upper bound text")?;
        let from = f64::from_bits(input.long("a range lower bound")?.cast_unsigned());
        let to = f64::from_bits(input.long("a range upper bound")?.cast_unsigned());
        ranges.push(RangeAsk {
            key,
            from,
            from_text,
            to,
            to_text,
        });
    }
    let keyed = input.bool("keyed")?;
    Ok(RangesRequest { ranges, keyed })
}

/// Writes `r` after a range builder's values-source fields.
///
/// # Errors
///
/// [`WireError::TooLarge`] past the writer's limit.
pub fn write(r: &RangesRequest, out: &mut Output<'_>) -> Result<(), WireError> {
    out.count(r.ranges.len())?;
    for a in &r.ranges {
        out.optional_string(a.key.as_deref())?;
        out.optional_string(a.from_text.as_deref())?;
        out.optional_string(a.to_text.as_deref())?;
        out.bytes(&a.from.to_bits().to_be_bytes())?;
        out.bytes(&a.to.to_bits().to_be_bytes())?;
    }
    out.bool(r.keyed)
}

/// One bucket of a shard's `InternalRange`.
#[derive(Debug, Clone, PartialEq)]
pub struct RangeBucketAnswer {
    /// The key, asked for or generated from the bounds.
    pub key: String,
    /// The lower bound.
    pub from: f64,
    /// The upper bound.
    pub to: f64,
    /// The documents in the range.
    pub count: i64,
    /// The sub-aggregation results; empty without sub-aggregations.
    pub subs: Vec<SubAnswer>,
}

/// A shard's `InternalRange` or `InternalDateRange`.
#[derive(Debug, Clone, PartialEq)]
pub struct RangeAnswer {
    /// Named `date_range` rather than `range`.
    pub date_range: bool,
    /// `keyed`.
    pub keyed: bool,
    /// One bucket a range of the request.
    pub buckets: Vec<RangeBucketAnswer>,
}

impl RangeAnswer {
    /// The answer's writeable name.
    pub fn name(&self) -> &'static str {
        if self.date_range {
            "date_range"
        } else {
            "range"
        }
    }

    /// `InternalRange.doWriteTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, date: bool, out: &mut Output<'_>) -> Result<(), WireError> {
        write_format(out, date)?;
        out.bool(self.keyed)?;
        out.count(self.buckets.len())?;
        for b in &self.buckets {
            out.string(&b.key)?;
            out.bytes(&b.from.to_bits().to_be_bytes())?;
            out.bytes(&b.to.to_bits().to_be_bytes())?;
            out.vlong(b.count)?;
            out.count(b.subs.len())?;
            for sub in &b.subs {
                write_one(&sub.name, &sub.value, sub.date, out)?;
            }
        }
        Ok(())
    }
}

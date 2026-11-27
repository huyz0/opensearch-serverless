//! The `date_histogram` aggregation of a forwarded search:
//! `DateHistogramAggregationBuilder`'s own fields, and the shard's
//! `InternalDateHistogram` answering it.

use super::Refusal;
use super::aggs::write_format;
use super::aggs_terms::{OrderSpec, SubAnswer};
use crate::error::WireError;
use crate::stream::{Input, Output};

/// Which interval a `date_histogram` was given (`IntervalTypeEnum`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntervalType {
    /// `fixed_interval`.
    Fixed,
    /// `calendar_interval`.
    Calendar,
}

/// What a `date_histogram` asks beyond its field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateHistogramRequest {
    /// `order`.
    pub order: OrderSpec,
    /// `keyed`.
    pub keyed: bool,
    /// `min_doc_count`.
    pub min_doc_count: i64,
    /// The interval as written (`1d`, `30s`).
    pub interval: String,
    /// Whether it is a calendar or a fixed interval.
    pub interval_type: IntervalType,
    /// `offset`, in milliseconds.
    pub offset: i64,
}

fn none(input: &mut Input<'_>, what: &'static str) -> Result<(), Refusal> {
    if input.bool(what)? {
        return Err(Refusal::Unsupported(what));
    }
    Ok(())
}

/// Reads the fields after a `date_histogram` builder's values-source fields.
///
/// # Errors
///
/// [`Refusal`] for malformed bytes or an option this node does not run.
pub fn read(input: &mut Input<'_>) -> Result<DateHistogramRequest, Refusal> {
    let order = OrderSpec::read(input)?;
    let keyed = input.bool("keyed")?;
    let min_doc_count = input.vlong("min doc count")?;
    let interval = input
        .optional_string("an interval")?
        .ok_or(Refusal::Unsupported("a date histogram without an interval"))?;
    let interval_type = match input.vint("an interval type")? {
        1 => IntervalType::Fixed,
        2 => IntervalType::Calendar,
        _ => return Err(Refusal::Unsupported("a legacy interval")),
    };
    let offset = input.long("an offset")?;
    none(input, "extended bounds")?;
    none(input, "hard bounds")?;
    Ok(DateHistogramRequest {
        order,
        keyed,
        min_doc_count,
        interval,
        interval_type,
        offset,
    })
}

/// Writes `h` after a `date_histogram` builder's values-source fields.
///
/// # Errors
///
/// [`WireError::TooLarge`] past the writer's limit.
pub fn write(h: &DateHistogramRequest, out: &mut Output<'_>) -> Result<(), WireError> {
    h.order.write(out)?;
    out.bool(h.keyed)?;
    out.vlong(h.min_doc_count)?;
    out.optional_string(Some(&h.interval))?;
    out.vint(match h.interval_type {
        IntervalType::Fixed => 1,
        IntervalType::Calendar => 2,
    })?;
    out.bytes(&h.offset.to_be_bytes())?;
    out.bool(false)?; // extended bounds
    out.bool(false) // hard bounds
}

/// The rounding of an answer (`Rounding.writeTo`), in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundingWire {
    /// `TimeUnitRounding`, by `DateTimeUnit`'s id: week 1, year 2, quarter 3,
    /// month 4, day 5, hour 6, minute 7, second 8.
    Unit(u8),
    /// `TimeIntervalRounding`, in milliseconds.
    Interval(i64),
}

/// A shard's `InternalDateHistogram`.
#[derive(Debug, Clone, PartialEq)]
pub struct DateHistogramAnswer {
    /// The order asked for.
    pub order: OrderSpec,
    /// `min_doc_count`.
    pub min_doc_count: i64,
    /// The rounding of the empty buckets, written when the minimum is 0.
    pub rounding: RoundingWire,
    /// What each sub-aggregation holds for an empty bucket, written with the
    /// rounding.
    pub empty_subs: Vec<SubAnswer>,
    /// `offset`, in milliseconds.
    pub offset: i64,
    /// `keyed`.
    pub keyed: bool,
    /// The buckets' keys, in milliseconds, ascending.
    pub keys: Vec<i64>,
    /// The buckets' document counts, one a key.
    pub counts: Vec<i64>,
    /// The buckets' sub-aggregation results, one list a key; empty without
    /// sub-aggregations.
    pub subs: Vec<Vec<SubAnswer>>,
}

impl DateHistogramAnswer {
    /// `InternalDateHistogram.doWriteTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        self.order.write(out)?;
        out.vlong(self.min_doc_count)?;
        if self.min_doc_count == 0 {
            // `EmptyBucketInfo`: the rounding, the empty sub-aggregations, and
            // the bounds, of which there are none.
            match self.rounding {
                RoundingWire::Unit(id) => {
                    out.byte(1)?;
                    out.byte(id)?;
                }
                RoundingWire::Interval(ms) => {
                    out.byte(2)?;
                    out.vlong(ms)?;
                }
            }
            out.string("Z")?;
            out.count(self.empty_subs.len())?;
            for s in &self.empty_subs {
                super::aggs::write_one(&s.name, &s.value, s.date, out)?;
            }
            out.bool(false)?;
        }
        out.bytes(&self.offset.to_be_bytes())?;
        write_format(out, true)?;
        out.bool(self.keyed)?;
        out.count(self.keys.len())?;
        for (i, key) in self.keys.iter().enumerate() {
            out.bytes(&key.to_be_bytes())?;
            out.vlong(self.counts.get(i).copied().unwrap_or(0))?;
            let subs = self.subs.get(i).map_or(&[][..], Vec::as_slice);
            out.count(subs.len())?;
            for s in subs {
                super::aggs::write_one(&s.name, &s.value, s.date, out)?;
            }
        }
        Ok(())
    }
}

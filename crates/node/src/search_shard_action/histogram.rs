//! A shard's `date_histogram` result as the forwarded answer's
//! `InternalDateHistogram`, with each bucket's sub-aggregations.

use super::answer_value::value;
use super::refuse;
use oss_reader::aggs::bucket::date_histogram::{DateHistogramResult, DateUnit, RoundingKind};
use oss_reader::aggs::partial::{Format, Value as Part};
use oss_transport::actions::search_shard::{
    AggRequest, DateHistogramAnswer, RoundingWire, SubAnswer,
};
use oss_transport::exception::RemoteException;

/// `Rounding.DateTimeUnit`'s id on the wire.
fn unit_id(unit: DateUnit) -> u8 {
    match unit {
        DateUnit::Week => 1,
        DateUnit::Year => 2,
        DateUnit::Quarter => 3,
        DateUnit::Month => 4,
        DateUnit::Day => 5,
        DateUnit::Hour => 6,
        DateUnit::Minute => 7,
        DateUnit::Second => 8,
    }
}

/// One bucket's results.
fn results(
    meta: &[(String, Format)],
    values: &[Part],
    asked: &[AggRequest],
) -> Result<Vec<SubAnswer>, RemoteException> {
    // ALLOC: one result a sub-aggregation, at most the request's.
    let mut one = Vec::with_capacity(meta.len());
    for ((name, format), v) in meta.iter().zip(values) {
        one.push(SubAnswer {
            name: name.clone(),
            value: value(name, v, *format, asked)?,
            date: *format == Format::Date,
        });
    }
    Ok(one)
}

/// A `date_histogram` over a date field.
///
/// # Errors
///
/// [`RemoteException`] for a request that is not a `date_histogram`, a zone
/// other than UTC, or a sub-aggregation result of a kind this node does not
/// answer.
pub fn answer(
    r: &DateHistogramResult,
    req: &AggRequest,
) -> Result<DateHistogramAnswer, RemoteException> {
    let options = req.histogram.as_ref().ok_or_else(|| {
        refuse(
            "a date histogram answer for a request that is not one",
            "INTERNAL_SERVER_ERROR",
        )
    })?;
    if r.spec.rounding.zone_ms != 0 || r.spec.rounding.region.is_some() {
        return Err(refuse(
            "a forwarded date histogram in a time zone is not supported by this node",
            "BAD_REQUEST",
        ));
    }
    let rounding = match r.spec.rounding.kind {
        RoundingKind::Unit(u) => RoundingWire::Unit(unit_id(u)),
        RoundingKind::Interval(ms) => RoundingWire::Interval(ms),
    };
    let mut subs = Vec::new();
    let mut empty_subs = Vec::new();
    if !r.subs.meta.is_empty() {
        // ALLOC: one list a bucket, at most the bucket limit.
        subs.reserve(r.subs.lists.len());
        for list in &r.subs.lists {
            subs.push(results(&r.subs.meta, list, &req.subs)?);
        }
        empty_subs = results(&r.subs.meta, &r.subs.empty, &req.subs)?;
    }
    Ok(DateHistogramAnswer {
        order: options.order.clone(),
        min_doc_count: options.min_doc_count,
        rounding,
        empty_subs,
        offset: options.offset,
        keyed: options.keyed,
        keys: r.buckets.iter().map(|b| b.0).collect(),
        counts: r
            .buckets
            .iter()
            .map(|b| i64::try_from(b.1).unwrap_or(i64::MAX))
            .collect(),
        subs,
    })
}

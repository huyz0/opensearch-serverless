//! A shard's `range` or `date_range` result as the forwarded answer's
//! `InternalRange` or `InternalDateRange`, with each bucket's
//! sub-aggregations.

use super::answer_value::value;
use super::refuse;
use oss_reader::aggs::bucket::range::RangeResult;
use oss_reader::aggs::bucket::range_render::key_of;
use oss_reader::aggs::partial::{Format, Value as Part};
use oss_transport::actions::search_shard::{
    AggKind, AggRequest, RangeAnswer, RangeBucketAnswer, SubAnswer,
};
use oss_transport::exception::RemoteException;

/// One bucket's sub-aggregation results.
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

/// A `range` or `date_range` over a field of `format`.
///
/// # Errors
///
/// [`RemoteException`] for a request that is not a range, or a
/// sub-aggregation result of a kind this node does not answer.
pub fn answer(
    r: &RangeResult,
    format: Format,
    req: &AggRequest,
) -> Result<RangeAnswer, RemoteException> {
    if !matches!(req.kind, AggKind::Range | AggKind::DateRange) {
        return Err(refuse(
            "a range answer for a request that is not one",
            "INTERNAL_SERVER_ERROR",
        ));
    }
    // ALLOC: one bucket a range, at most the request's.
    let mut buckets = Vec::with_capacity(r.buckets.len());
    for (i, b) in r.buckets.iter().enumerate() {
        let key = key_of(format, r.format.as_ref(), b)
            .ok()
            .and_then(|k| String::from_utf8(k).ok())
            .ok_or_else(|| refuse("a range key that is not text", "INTERNAL_SERVER_ERROR"))?;
        let subs = match r.subs.get(i) {
            Some(list) if !r.sub_meta.is_empty() => results(&r.sub_meta, list, &req.subs)?,
            _ => Vec::new(),
        };
        buckets.push(RangeBucketAnswer {
            key,
            from: b.from,
            to: b.to,
            count: i64::try_from(b.doc_count).unwrap_or(i64::MAX),
            subs,
        });
    }
    Ok(RangeAnswer {
        date_range: req.kind == AggKind::DateRange,
        keyed: r.keyed,
        buckets,
    })
}

//! A shard's `terms` result as the forwarded answer's `StringTerms` or
//! `LongTerms`, with each bucket's sub-aggregations.

use super::answer_value::value;
use super::refuse;
use oss_reader::aggs::bucket::terms::{TermsOrder, TermsSpec};
use oss_reader::aggs::partial::{Format, Value as Part};
use oss_transport::actions::search_shard::{AggRequest, Keys, Order, SubAnswer, TermsAnswer};
use oss_transport::exception::RemoteException;

fn int(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn long(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// What the buckets hold, around their keys.
struct Shape<'a> {
    spec: &'a TermsSpec,
    other_doc_count: u64,
    doc_count_error: i64,
    bucket_errors: &'a [i64],
    counts: Vec<u64>,
    /// The name and format of each sub-aggregation.
    meta: &'a [(String, Format)],
    /// One list of values a bucket, empty without sub-aggregations.
    lists: &'a [Vec<Part>],
}

/// The answer's shape, around its keys.
fn answer(req: &AggRequest, shape: &Shape<'_>, keys: Keys) -> Result<TermsAnswer, RemoteException> {
    let options = req.terms.as_ref().ok_or_else(|| {
        refuse(
            "a terms answer for a request that is not a terms",
            "INTERNAL_SERVER_ERROR",
        )
    })?;
    let spec = shape.spec;
    let mut subs = Vec::new();
    if !shape.meta.is_empty() {
        // ALLOC: one list a bucket, at most the shard size.
        subs.reserve(shape.lists.len());
        for list in shape.lists {
            // ALLOC: one result a sub-aggregation, at most the request's.
            let mut one = Vec::with_capacity(shape.meta.len());
            for ((name, format), v) in shape.meta.iter().zip(list) {
                one.push(SubAnswer {
                    name: name.clone(),
                    value: value(name, v, *format, &req.subs)?,
                    date: *format == Format::Date,
                });
            }
            subs.push(one);
        }
    }
    Ok(TermsAnswer {
        // The shard lists its buckets by key, descending only for that order.
        reduce_order: if spec.order == TermsOrder::KeyDesc {
            Order::KeyDesc
        } else {
            Order::KeyAsc
        },
        order: options.order.clone(),
        size: int(spec.size),
        min_doc_count: long(spec.min_doc_count),
        doc_count_error: shape.doc_count_error,
        shard_size: int(spec.shard_size),
        show_error: spec.show_error,
        other_doc_count: long(shape.other_doc_count),
        keys,
        counts: shape.counts.iter().map(|c| long(*c)).collect(),
        errors: shape.bucket_errors.to_vec(),
        subs,
    })
}

/// A `terms` over a number or a date.
///
/// # Errors
///
/// [`RemoteException`] for a sub-aggregation result of a kind this node does
/// not answer.
pub fn numeric(
    r: &oss_reader::aggs::bucket::terms::TermsResult,
    req: &AggRequest,
    format: Format,
) -> Result<TermsAnswer, RemoteException> {
    let shape = Shape {
        spec: &r.spec,
        other_doc_count: r.other_doc_count,
        doc_count_error: r.doc_count_error,
        bucket_errors: &r.bucket_errors,
        counts: r.buckets.iter().map(|b| b.1).collect(),
        meta: &r.subs.meta,
        lists: &r.subs.lists,
    };
    let keys = if format == Format::Double {
        // The engine counts a double by its sortable long.
        Keys::Double(
            r.buckets
                .iter()
                .map(|b| oss_reader::aggs::bucket::terms_collect::double_of_key(b.0))
                .collect(),
        )
    } else {
        Keys::Long(r.buckets.iter().map(|b| b.0).collect())
    };
    answer(req, &shape, keys)
}

/// A `terms` over a keyword.
///
/// # Errors
///
/// [`RemoteException`] for a sub-aggregation result of a kind this node does
/// not answer.
pub fn keyword(
    r: &oss_reader::aggs::bucket::keyword_terms::KeywordTermsResult,
    req: &AggRequest,
) -> Result<TermsAnswer, RemoteException> {
    let shape = Shape {
        spec: &r.spec,
        other_doc_count: r.other_doc_count,
        doc_count_error: r.doc_count_error,
        bucket_errors: &r.bucket_errors,
        counts: r.buckets.iter().map(|b| b.1).collect(),
        meta: &r.sub_meta,
        lists: &r.subs,
    };
    answer(
        req,
        &shape,
        Keys::Text(r.buckets.iter().map(|b| b.0.clone()).collect()),
    )
}

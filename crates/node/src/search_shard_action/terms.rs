//! A shard's `terms` result as the forwarded answer's `StringTerms` or
//! `LongTerms`.

use super::refuse;
use oss_reader::aggs::bucket::terms::{TermsOrder, TermsSpec};
use oss_transport::actions::search_shard::{Keys, Order, OrderSpec, TermsAnswer};
use oss_transport::exception::RemoteException;

fn int(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn long(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// The answer's shape, around its keys and counts.
fn answer(
    spec: &TermsSpec,
    order: OrderSpec,
    other_doc_count: u64,
    doc_count_error: i64,
    bucket_errors: &[i64],
    keys: Keys,
    counts: &[u64],
) -> TermsAnswer {
    TermsAnswer {
        // The shard lists its buckets by key, descending only for that order.
        reduce_order: if spec.order == TermsOrder::KeyDesc {
            Order::KeyDesc
        } else {
            Order::KeyAsc
        },
        order,
        size: int(spec.size),
        min_doc_count: long(spec.min_doc_count),
        doc_count_error,
        shard_size: int(spec.shard_size),
        show_error: spec.show_error,
        other_doc_count: long(other_doc_count),
        keys,
        counts: counts.iter().map(|c| long(*c)).collect(),
        errors: bucket_errors.to_vec(),
    }
}

fn no_subs(has: bool) -> Result<(), RemoteException> {
    if has {
        return Err(refuse(
            "a forwarded terms with sub-aggregations is not supported by this node",
            "BAD_REQUEST",
        ));
    }
    Ok(())
}

/// A `terms` over a number or a date.
///
/// # Errors
///
/// [`RemoteException`] for a result with sub-aggregations.
pub fn numeric(
    r: &oss_reader::aggs::bucket::terms::TermsResult,
    order: OrderSpec,
) -> Result<TermsAnswer, RemoteException> {
    no_subs(!r.subs.meta.is_empty())?;
    let counts: Vec<u64> = r.buckets.iter().map(|b| b.1).collect();
    Ok(answer(
        &r.spec,
        order,
        r.other_doc_count,
        r.doc_count_error,
        &r.bucket_errors,
        Keys::Long(r.buckets.iter().map(|b| b.0).collect()),
        &counts,
    ))
}

/// A `terms` over a keyword.
///
/// # Errors
///
/// [`RemoteException`] for a result with sub-aggregations.
pub fn keyword(
    r: &oss_reader::aggs::bucket::keyword_terms::KeywordTermsResult,
    order: OrderSpec,
) -> Result<TermsAnswer, RemoteException> {
    no_subs(!r.sub_meta.is_empty())?;
    let counts: Vec<u64> = r.buckets.iter().map(|b| b.1).collect();
    Ok(answer(
        &r.spec,
        order,
        r.other_doc_count,
        r.doc_count_error,
        &r.bucket_errors,
        Keys::Text(r.buckets.iter().map(|b| b.0.clone()).collect()),
        &counts,
    ))
}

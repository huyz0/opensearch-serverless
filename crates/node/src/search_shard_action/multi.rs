//! A shard's `multi_terms` result as the forwarded answer's
//! `InternalMultiTerms`.

use super::refuse;
use oss_reader::aggs::bucket::multi::{MultiPart, MultiResult};
use oss_transport::actions::search_shard::{AggRequest, MultiAnswer, MultiTerm};
use oss_transport::exception::RemoteException;

fn int(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn long(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// The answer for `r`, the result of the request `req`.
///
/// # Errors
///
/// [`RemoteException`] for a request that is not a `multi_terms`.
pub fn answer(r: &MultiResult, req: &AggRequest) -> Result<MultiAnswer, RemoteException> {
    let multi = req.multi.as_ref().ok_or_else(|| {
        refuse(
            "a multi_terms answer for a request that is not a multi_terms",
            "INTERNAL_SERVER_ERROR",
        )
    })?;
    Ok(MultiAnswer {
        order: multi.options.order.clone(),
        size: int(r.spec.size),
        min_doc_count: long(r.spec.min_doc_count),
        doc_count_error: r.doc_count_error,
        shard_size: int(r.spec.shard_size),
        other_doc_count: long(r.other_doc_count),
        terms: multi.fields.len(),
        keys: r
            .buckets
            .iter()
            .map(|(key, _)| {
                key.iter()
                    .map(|part| match part {
                        MultiPart::Text(t) => MultiTerm::Text(t.clone()),
                        MultiPart::Long(n) => MultiTerm::Long(*n),
                        MultiPart::Double(n) => MultiTerm::Double(
                            oss_reader::aggs::bucket::terms_collect::double_of_key(*n),
                        ),
                    })
                    .collect()
            })
            .collect(),
        counts: r.buckets.iter().map(|b| long(b.1)).collect(),
        show_error: r.spec.show_error,
        errors: r.bucket_errors.clone(),
    })
}

//! A shard's `rare_terms` result as the forwarded answer's `StringRareTerms`
//! or `LongRareTerms`, the terms that were too common as hashes in the
//! filter OpenSearch keeps.

use super::refuse;
use oss_reader::aggs::bucket::rare::{RareKey, RareResult};
use oss_transport::actions::search_shard::{FILTER_THRESHOLD, RareAnswer, RareKeys};
use oss_transport::exception::RemoteException;

/// The answer for `r`: the rare terms by count then term, and the hashes of
/// the terms held by more documents than the maximum.
///
/// # Errors
///
/// [`RemoteException`] for more common terms than the filter holds as a set
/// (past that OpenSearch's filter is a cuckoo filter, which this node does not
/// write).
pub fn answer(r: &RareResult) -> Result<RareAnswer, RemoteException> {
    if r.excluded.len() > FILTER_THRESHOLD {
        return Err(refuse(
            "a rare_terms with more common terms than its filter holds as a set is not \
             supported by this node",
            "BAD_REQUEST",
        ));
    }
    let mut buckets: Vec<&(RareKey, u64)> = r.buckets.iter().collect();
    buckets.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    let counts = buckets
        .iter()
        .map(|b| i64::try_from(b.1).unwrap_or(i64::MAX))
        .collect();
    let keys = if r.long {
        RareKeys::Long(
            buckets
                .iter()
                .map(|b| match &b.0 {
                    RareKey::Long(n) => *n,
                    RareKey::Text(_) => 0,
                })
                .collect(),
        )
    } else {
        RareKeys::Text(
            buckets
                .iter()
                .map(|b| match &b.0 {
                    RareKey::Text(t) => t.clone(),
                    RareKey::Long(n) => n.to_string(),
                })
                .collect(),
        )
    };
    Ok(RareAnswer {
        max_doc_count: i64::try_from(r.spec.max_doc_count).unwrap_or(i64::MAX),
        keys,
        counts,
        excluded: r.excluded.iter().map(RareKey::filter_hash).collect(),
    })
}

//! Running a keyword `terms` on one shard: the engine's keyword `terms` node
//! (`GlobalOrdinalsStringTermsAggregator`) over the field's global ordinals,
//! its result as `StringTerms` holds it, and the memory the ordinals take.

use lucene_aggs::bucket_aggs::AggNode;
use lucene_search::global_ords::keyword_column;

use crate::OpenShard;
use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::bucket::keyword_terms::KeywordTermsResult;
use crate::aggs::bucket::terms::TermsSpec;
use oss_blob::BlobStore;

/// What one term of a segment's dictionary holds while the search runs: its
/// entry in the segment's map to global ordinals (8 bytes), the global
/// ordinal's first segment (4) and its ordinal there (8), and its count
/// (8), with 12 bytes for the engine's working arrays.
pub const ORD_BYTES: usize = 40;

/// What each kept bucket holds beyond its key: the engine's term and the
/// result's, each a 24-byte header, and the count and map entry a reduce
/// adds (16 bytes). The key itself is held three times: by the engine, by
/// the result and by a reduce.
pub const BUCKET_BYTES: usize = 2 * 24 + 16;

/// How many times a kept bucket's key is alive at once.
pub const KEY_COPIES: usize = 3;

/// The most bytes a keyword `terms` of `field` and `spec` holds on `shard`:
/// [`ORD_BYTES`] for each term of each segment's dictionary (their number is
/// in the segments' metadata, so nothing is read to size it), the merge's two
/// term buffers a segment, and the key of each kept bucket (at most the
/// bucket limit, and no more than the terms there are) at [`KEY_COPIES`]
/// copies of the longest term of any segment and [`BUCKET_BYTES`].
pub fn reserved_bytes<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    field: &str,
    spec: &TermsSpec,
) -> usize {
    let mut terms = 0usize;
    let mut segments = 0usize;
    let mut longest = 0usize;
    for segment in shard.reader.segment_readers() {
        // A dictionary that cannot be read fails the search itself.
        if let Ok(Some((_, _, entry))) = keyword_column(segment, field) {
            segments = segments.saturating_add(1);
            terms = terms.saturating_add(usize::try_from(entry.terms_dict_size).unwrap_or(0));
            longest = longest.max(usize::try_from(entry.max_term_length).unwrap_or(0));
        }
    }
    let kept = spec.shard_size.min(BUCKET_LIMIT).min(terms);
    terms
        .saturating_mul(ORD_BYTES)
        .saturating_add(segments.saturating_mul(longest).saturating_mul(2))
        .saturating_add(
            kept.saturating_mul(
                longest
                    .saturating_mul(KEY_COPIES)
                    .saturating_add(BUCKET_BYTES),
            ),
        )
}

/// The engine's node for a `terms` of the keyword field `field`.
pub fn node(field: &str, spec: &TermsSpec) -> AggNode {
    AggNode::Terms {
        // ALLOC: one field name an aggregation, bounded by the request's.
        field: field.to_owned(),
        missing: None,
        accepted: None,
        shard_size: spec.shard_size,
        order: spec.order,
        zero_buckets: false,
        // ALLOC: empty; sub-aggregations are refused.
        subs: Vec::new(),
    }
}

/// A shard's result with no bucket: an unmapped field, or no match.
pub fn empty(spec: &TermsSpec) -> KeywordTermsResult {
    // ALLOC: empty.
    from_engine(spec, 0, Vec::new())
}

/// The result of the engine's first owning bucket: the documents of the
/// buckets it did not keep and the kept ones by term. A shard has no
/// slice-level error, so its `doc_count_error` is 0.
pub fn from_engine(
    spec: &TermsSpec,
    other: u64,
    buckets: Vec<(Vec<u8>, u64)>,
) -> KeywordTermsResult {
    KeywordTermsResult {
        // ALLOC: one spec a `terms`, bounded by the request's.
        spec: spec.clone(),
        other_doc_count: other,
        doc_count_error: 0,
        buckets: buckets
            .into_iter()
            // ALLOC: one term a kept bucket, at most the shard size.
            .map(|(term, count)| (String::from_utf8_lossy(&term).into_owned(), count))
            // ALLOC: the kept buckets, as above.
            .collect(),
    }
}

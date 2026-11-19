//! Running a keyword `terms` on one shard: the engine's keyword `terms` node
//! (`GlobalOrdinalsStringTermsAggregator`) over the field's global ordinals,
//! its result as `StringTerms` holds it, and the memory the ordinals take.

use lucene_aggs::bucket_aggs::{AcceptedOrds, AggNode};
use lucene_codecs::regexp::RegexpPattern;
use lucene_codecs::terms_dict::TermsCursor;
use lucene_search::global_ords::{GlobalOrds, keyword_column};

use crate::OpenShard;
use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::bucket::keyword_options::TermFilter;
use crate::aggs::bucket::keyword_terms::KeywordTermsResult;
use crate::aggs::bucket::terms::TermsSpec;
use crate::failure::Failure;
use crate::search::ShardError;
use oss_blob::BlobStore;

/// What each term of the dictionaries holds while `min_doc_count: 0` lists
/// the empty buckets: a candidate bucket (24 bytes) and its seen flag.
pub const ZERO_BUCKET_BYTES: usize = 32;

/// The most characters of an `include` or `exclude` pattern
/// (`IncludeExclude.DEFAULT_MAX_REGEX_LENGTH`).
const MAX_REGEX_LENGTH: usize = 1000;

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

/// How many times a kept bucket's sub-aggregation value is alive at once.
pub const SUB_COPIES: usize = 3;

/// The most bytes a keyword `terms` of `field` and `spec` holds on `shard`:
/// [`ORD_BYTES`] for each term of each segment's dictionary (their number is
/// in the segments' metadata, so nothing is read to size it), the merge's two
/// term buffers a segment, and the key of each kept bucket (at most the
/// bucket limit, and no more than the terms there are) at [`KEY_COPIES`]
/// copies of the longest term of any segment and [`BUCKET_BYTES`], and
/// `subs` sub-aggregations' states and values.
pub fn reserved_bytes<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    field: &str,
    spec: &TermsSpec,
    subs: usize,
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
    // `min_doc_count: 0` makes every term a candidate bucket; a filter's
    // accepted bits are built beside a shifted copy and read terms as long
    // as the longest.
    let zero = if spec.min_doc_count == 0 {
        terms.saturating_mul(ZERO_BUCKET_BYTES)
    } else {
        0
    };
    let filter = if spec.keyword.filters() {
        terms
            .div_ceil(8)
            .saturating_add(8)
            .saturating_mul(2)
            .saturating_add(longest)
    } else {
        0
    };
    // Each sub-aggregation keeps an engine state for every term the search
    // counts, and a value for each kept bucket in the engine's result, the
    // shard's and a reduce's.
    let sub_states = terms
        .saturating_mul(subs)
        .saturating_mul(std::mem::size_of::<lucene_aggs::metric_aggs::MetricState>());
    let sub_values = kept
        .saturating_mul(subs)
        .saturating_mul(SUB_COPIES)
        .saturating_mul(std::mem::size_of::<crate::aggs::partial::Value>());
    terms
        .saturating_mul(ORD_BYTES)
        .saturating_add(sub_states)
        .saturating_add(sub_values)
        .saturating_add(segments.saturating_mul(longest).saturating_mul(2))
        .saturating_add(
            kept.saturating_mul(
                longest
                    .saturating_mul(KEY_COPIES)
                    .saturating_add(BUCKET_BYTES),
            ),
        )
        .saturating_add(zero)
        .saturating_add(filter)
}

/// The engine's node for a `terms` of the keyword field `field`, with the
/// accepted ordinals of its `include` and `exclude` ([`accepted_ords`]).
pub fn node(field: &str, spec: &TermsSpec, accepted: Option<AcceptedOrds>) -> AggNode {
    AggNode::Terms {
        // ALLOC: one field name an aggregation, bounded by the request's.
        field: field.to_owned(),
        // ALLOC: one term an aggregation, bounded by the request's.
        missing: spec.keyword.missing.as_ref().map(|m| m.as_bytes().to_vec()),
        accepted,
        shard_size: spec.shard_size,
        order: spec.order,
        zero_buckets: spec.min_doc_count == 0,
        // ALLOC: empty; `keyword_subs` adds the sub-aggregations.
        subs: Vec::new(),
    }
}

fn bad_request(reason: String) -> ShardError {
    ShardError {
        status: 400,
        failure: Failure::plain("illegal_argument_exception", reason),
    }
}

fn engine_error(e: &dyn std::fmt::Display) -> ShardError {
    ShardError {
        status: 500,
        // ALLOC: error path; one message.
        failure: Failure::opensearch("exception", e.to_string()),
    }
}

/// One side of `include` or `exclude` ready to test terms
/// (`IncludeExclude.toAutomaton`).
enum Matcher {
    Regex(Box<RegexpPattern>),
    Set(Vec<Vec<u8>>),
}

impl Matcher {
    fn new(filter: &TermFilter) -> Result<Self, ShardError> {
        match filter {
            TermFilter::Regex(pattern) => {
                // `validateRegExpStringLength`, in Java's UTF-16 units.
                let length = pattern.encode_utf16().count();
                if length > MAX_REGEX_LENGTH {
                    // ALLOC: error path; one message.
                    return Err(bad_request(format!(
                        "The length of regex [{length}] used in the request has exceeded the \
                         allowed maximum of [{MAX_REGEX_LENGTH}]. This maximum can be set by \
                         changing the [index.max_regex_length] index level setting."
                    )));
                }
                // ALLOC: error path; one message.
                let compiled =
                    RegexpPattern::parse(pattern).map_err(|e| bad_request(e.to_string()))?;
                // A pattern too large to determinize would be matched by a
                // backtracking fallback that answers "no match" past its
                // budget; OpenSearch fails it instead.
                if compiled.dfa_matches(b"").is_none() {
                    return Err(bad_request(
                        // ALLOC: error path; one message.
                        "a pattern of an [include] or [exclude] too complex to determinize is \
                         not supported by this node yet"
                            .to_owned(),
                    ));
                }
                // ALLOC: one compiled pattern a filter, bounded by the request's.
                Ok(Matcher::Regex(Box::new(compiled)))
            }
            // ALLOC: one term a listed value; the body's size bounds them.
            TermFilter::Set(terms) => Ok(Matcher::Set(
                terms.iter().map(|t| t.as_bytes().to_vec()).collect(),
            )),
        }
    }

    fn matches(&self, term: &[u8]) -> bool {
        match self {
            Matcher::Regex(p) => p.dfa_matches(term).unwrap_or(false),
            Matcher::Set(terms) => terms.binary_search_by(|t| t.as_slice().cmp(term)).is_ok(),
        }
    }
}

fn set_bit(bits: &mut [u64], ord: usize) {
    if let Some(word) = bits.get_mut(ord >> 6) {
        *word |= 1 << (ord & 63);
    }
}

fn get_bit(bits: &[u64], ord: usize) -> bool {
    bits.get(ord >> 6).is_some_and(|w| w >> (ord & 63) & 1 == 1)
}

/// The global ordinals `include` and `exclude` accept
/// (`IncludeExclude.OrdinalsFilter.acceptedGlobalOrdinals`), numbered with
/// the `missing` term among them where it sorts when no segment holds it:
/// `None` when the aggregation has neither. Each segment's dictionary is
/// read once in order, and its memory is what [`reserved_bytes`] counts.
pub fn accepted_ords<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    field: &str,
    ords: &GlobalOrds,
    spec: &TermsSpec,
) -> Result<Option<AcceptedOrds>, ShardError> {
    if !spec.keyword.filters() {
        return Ok(None);
    }
    let include = spec
        .keyword
        .include
        .as_ref()
        .map(Matcher::new)
        .transpose()?;
    let exclude = spec
        .keyword
        .exclude
        .as_ref()
        .map(Matcher::new)
        .transpose()?;
    let accepts = |term: &[u8]| {
        include.as_ref().is_none_or(|m| m.matches(term))
            && !exclude.as_ref().is_some_and(|m| m.matches(term))
    };
    let missing = spec.keyword.missing.as_ref().map(|m| m.as_bytes());
    let count = ords.value_count();
    // ALLOC: one bit a global ordinal, reserved by `reserved_bytes`.
    let mut bits = vec![0u64; count.div_ceil(64)];
    let mut before_missing = 0usize;
    let mut holds_missing = false;
    for (index, segment) in shard.reader.segment_readers().iter().enumerate() {
        let Some((_, data, entry)) =
            keyword_column(segment, field).map_err(|e| engine_error(&e))?
        else {
            continue;
        };
        let to_global = ords.segment_map(index).unwrap_or_default();
        let mut cursor = TermsCursor::open(data, entry).map_err(|e| engine_error(&e))?;
        let mut segment_ord = 0usize;
        while let Some(term) = cursor.next_term().map_err(|e| engine_error(&e))? {
            let global = to_global.get(segment_ord).copied().unwrap_or(-1);
            segment_ord = segment_ord.saturating_add(1);
            // A term of several segments is read from the first.
            if ords.first_segment(global) != Some(index) {
                continue;
            }
            if accepts(term) {
                set_bit(&mut bits, usize::try_from(global).unwrap_or(usize::MAX));
            }
            if let Some(m) = missing {
                match term.cmp(m) {
                    std::cmp::Ordering::Less => before_missing = before_missing.saturating_add(1),
                    std::cmp::Ordering::Equal => holds_missing = true,
                    std::cmp::Ordering::Greater => {}
                }
            }
        }
    }
    let Some(m) = missing.filter(|_| !holds_missing) else {
        return Ok(Some(AcceptedOrds {
            count: count as u64,
            // ALLOC: the bits, as above.
            bits: bits.into(),
        }));
    };
    // The inserted term takes the ordinal `before_missing`: every later
    // ordinal moves up one.
    // ALLOC: the bits with one more, as above.
    let mut shifted = vec![0u64; count.saturating_add(1).div_ceil(64)];
    for ord in 0..count {
        if get_bit(&bits, ord) {
            set_bit(
                &mut shifted,
                if ord >= before_missing {
                    ord.saturating_add(1)
                } else {
                    ord
                },
            );
        }
    }
    if accepts(m) {
        set_bit(&mut shifted, before_missing);
    }
    Ok(Some(AcceptedOrds {
        count: (count as u64).saturating_add(1),
        bits: shifted.into(),
    }))
}

/// The shard-level result of a keyword `terms` from the engine's `result`.
pub fn value(
    spec: &TermsSpec,
    result: Option<lucene_aggs::bucket_aggs::AggResult>,
) -> Result<crate::aggs::partial::Value, ShardError> {
    match result {
        Some(lucene_aggs::bucket_aggs::AggResult::Terms { buckets, .. }) => {
            let (other, kept) = buckets.into_iter().next().unwrap_or_default();
            // ALLOC: the kept buckets of one `terms`, at most its shard size.
            Ok(crate::aggs::partial::Value::KeywordTerms(Box::new(
                from_engine(spec, other, kept),
            )))
        }
        _ => Err(engine_error(&"an aggregation result of another kind")),
    }
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
        // ALLOC: empty; `keyword_subs` fills them.
        sub_meta: Vec::new(),
        // ALLOC: empty, as above.
        subs: Vec::new(),
    }
}

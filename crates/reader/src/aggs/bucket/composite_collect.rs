//! Running a `composite` on one shard: the engine's `composite` node
//! (`CompositeAggregator`) and its result as `InternalComposite` holds it.

use std::collections::HashMap;
use std::sync::Arc;

use lucene_aggs::bucket_aggs::{
    AggNode, AggResult, CompositeAfter, CompositeKeyValue, CompositeSource, CompositeValues,
    MissingOrder as EngineMissing,
};
use lucene_aggs::metric_aggs::ValueKind;
use lucene_search::global_ords::GlobalOrds;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::bucket::composite::{
    AfterValue, CompositeResult, CompositeSpec, KeyPart, MissingOrder, SourceKind, SourceSpec,
};
use crate::aggs::bucket::histogram::canonical_bits;
use crate::aggs::bucket::keyword_collect::dictionary;
use crate::aggs::bucket::terms_collect::too_many_buckets;
use crate::aggs::metric::MetricAgg;
use crate::aggs::metric::resolve::{text_fielddata, type_name};
use crate::aggs::partial::Value;
use crate::failure::Failure;
use crate::mapping::FieldType;
use crate::search::ShardError;

/// What a kept bucket holds while collecting, besides its key: the count,
/// the engine's queue entry and its hash entry.
const BUCKET_BYTES: usize = 96;

/// What each source's value of a key holds: the value and, for a keyword,
/// the copies of its term the engine and the result keep.
const KEY_PART_BYTES: usize = 48;

fn bad_request(reason: String) -> ShardError {
    ShardError {
        status: 400,
        failure: Failure::plain("illegal_argument_exception", reason),
    }
}

fn spec_of(agg: &MetricAgg) -> CompositeSpec {
    agg.composite
        .as_deref()
        .cloned()
        .unwrap_or_else(|| CompositeSpec {
            // ALLOC: empty; every `composite` the parser made has its spec.
            sources: Vec::new(),
            size: 0,
            after: None,
        })
}

/// The bytes a `composite` holds while collecting beyond its field
/// readers: its kept keys, the request's and the result's.
pub fn state_bytes(spec: &CompositeSpec) -> usize {
    let key = spec
        .sources
        .len()
        .saturating_mul(KEY_PART_BYTES)
        .saturating_add(BUCKET_BYTES);
    spec.size
        .min(BUCKET_LIMIT)
        .saturating_mul(key)
        .saturating_mul(3)
}

/// The bytes the keyword sources of `agg` hold on `shard`: the kept keys
/// copy a term of at most the longest of the field.
pub fn reserved<S: BlobStore + 'static>(shard: &OpenShard<S>, agg: &MetricAgg) -> usize {
    let spec = spec_of(agg);
    spec.sources.iter().fold(0usize, |total, source| {
        let keyword = shard
            .meta
            .mapping
            .get(&source.field)
            .is_some_and(|f| f.ty == FieldType::Keyword);
        if !keyword {
            return total;
        }
        let (terms, _, longest) = dictionary(shard, &source.field);
        total
            .saturating_add(terms.saturating_mul(16))
            .saturating_add(
                spec.size
                    .min(BUCKET_LIMIT)
                    .saturating_mul(longest)
                    .saturating_mul(3),
            )
    })
}

/// [`reserved`] summed over the `composite` aggregations of `aggs`.
pub fn reserved_all<S: BlobStore + 'static>(shard: &OpenShard<S>, aggs: &[MetricAgg]) -> usize {
    aggs.iter()
        .filter(|agg| agg.composite.is_some())
        .fold(0usize, |total, agg| {
            total.saturating_add(reserved(shard, agg))
        })
}

/// A `composite` over more buckets than the bucket limit fails as OpenSearch
/// fails a reduce that makes them.
pub fn check_limit(agg: &MetricAgg) -> Result<(), ShardError> {
    match agg.composite.as_deref() {
        Some(spec) if spec.size > BUCKET_LIMIT => Err(too_many_buckets(spec.size, BUCKET_LIMIT)),
        _ => Ok(()),
    }
}

/// What a source reads of its field on this shard.
enum Reads {
    Keyword,
    Long,
}

/// How `source`'s field is read, `None` when it is unmapped; the errors of
/// a field the aggregation cannot read.
fn reads<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    source: &SourceSpec,
) -> Result<Option<Reads>, ShardError> {
    let Some(field) = shard.meta.mapping.get(&source.field) else {
        return Ok(None);
    };
    // ALLOC: error path; one message.
    let ty = type_name(&field.ty).to_owned();
    let reads = match (&field.ty, source.kind) {
        (FieldType::Text, _) => return Err(text_fielddata(&source.field)),
        (FieldType::Keyword, SourceKind::Terms) => Reads::Keyword,
        (FieldType::Int { .. } | FieldType::Long | FieldType::Date, _) => Reads::Long,
        (FieldType::Keyword, kind) => {
            let what = match kind {
                SourceKind::DateHistogram { .. } => "date_histogram",
                _ => "histogram",
            };
            // ALLOC: error path; one message.
            return Err(bad_request(format!(
                "Field [{}] of type [{ty}] is not supported for aggregation [{what}]",
                source.field
            )));
        }
        (FieldType::Unsupported(what), _) => {
            // ALLOC: error path; one message.
            return Err(bad_request(format!(
                "aggregating {what} [{}] is not supported by this node yet",
                source.field
            )));
        }
    };
    if !field.doc_values {
        // ALLOC: error path; one message.
        return Err(bad_request(format!(
            "Can't load fielddata on [{}] because fielddata is unsupported on fields of type \
             [{ty}]. Use doc values instead.",
            source.field
        )));
    }
    Ok(Some(reads))
}

/// Checks the sources of `agg` against `shard`'s mapping: whether it runs
/// (a source on an unmapped field with no `missing_bucket` has no key, so
/// none does).
pub fn runs<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
) -> Result<bool, ShardError> {
    let spec = spec_of(agg);
    let mut runs = true;
    for source in &spec.sources {
        if reads(shard, source)?.is_none() {
            if source.missing_bucket {
                // ALLOC: error path; one message.
                return Err(bad_request(format!(
                    "a [missing_bucket] source on the unmapped field [{}] of a [composite] \
                     aggregation is not supported by this node yet",
                    source.field
                )));
            }
            runs = false;
        }
    }
    Ok(runs)
}

/// The sortable long of a double, as the engine keeps a double key.
fn sortable(d: f64) -> i64 {
    let bits = canonical_bits(d);
    // ARITH: none; shifts and masks of a 64-bit pattern.
    bits ^ ((bits >> 63) & i64::MAX)
}

fn number_format(text: &str) -> ShardError {
    // ALLOC: error path; one message.
    bad_request(format!("For input string: \"{text}\""))
}

/// The engine's `after` for `spec`, with OpenSearch's checks of it.
fn after_of(
    spec: &CompositeSpec,
    kinds: &[Reads],
) -> Result<Option<Vec<CompositeAfter>>, ShardError> {
    let Some(after) = &spec.after else {
        return Ok(None);
    };
    if after.len() != spec.sources.len() {
        // ALLOC: error path; one message.
        return Err(bad_request(format!(
            "[after] has {} value(s) but [sources] has {}",
            after.len(),
            spec.sources.len()
        )));
    }
    // ALLOC: one entry a source, bounded by the request's.
    let mut out = Vec::with_capacity(spec.sources.len());
    for (source, kind) in spec.sources.iter().zip(kinds) {
        let name = &source.name;
        let Some((_, value)) = after.iter().find(|(n, _)| n == name) else {
            // ALLOC: error path; one message.
            return Err(bad_request(format!("Missing value for [after.{name}]")));
        };
        let text = match value {
            AfterValue::Null if source.missing_bucket => {
                out.push(CompositeAfter::Missing);
                continue;
            }
            AfterValue::Null => {
                // ALLOC: error path; one message.
                return Err(bad_request(format!(
                    "Invalid value for [after.{name}], expected comparable, got [null]"
                )));
            }
            AfterValue::Str(t) | AfterValue::Number(t) => t,
        };
        out.push(match (kind, source.kind) {
            // ALLOC: one term an `after`, bounded by the request's.
            (Reads::Keyword, _) => CompositeAfter::Term(text.as_bytes().to_vec()),
            (Reads::Long, SourceKind::Terms | SourceKind::DateHistogram { .. }) => {
                CompositeAfter::Value(
                    text.trim()
                        .parse::<i64>()
                        .map_err(|_| number_format(text))?,
                )
            }
            (Reads::Long, SourceKind::Histogram { .. }) => CompositeAfter::Value(sortable(
                text.trim()
                    .parse::<f64>()
                    .map_err(|_| number_format(text))?,
            )),
        });
    }
    Ok(Some(out))
}

/// The engine's node for the `composite` `agg`, building the ordinals of
/// each keyword source into `ords`; `segments` is how many segments the
/// shard searches.
pub fn node<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    ords: &mut HashMap<String, Arc<GlobalOrds>>,
    segments: usize,
) -> Result<AggNode, ShardError> {
    let spec = spec_of(agg);
    // ALLOC: one entry a source, bounded by the request's.
    let mut kinds = Vec::with_capacity(spec.sources.len());
    // ALLOC: as above.
    let mut sources = Vec::with_capacity(spec.sources.len());
    for source in &spec.sources {
        let Some(kind) = reads(shard, source)? else {
            // ALLOC: error path; one message.
            return Err(bad_request(format!(
                "the field [{}] of a [composite] source is unmapped",
                source.field
            )));
        };
        let values = match (&kind, source.kind) {
            (Reads::Keyword, _) => {
                if !ords.contains_key(&source.field) {
                    let built = shard.reader.global_ords(&source.field).map_err(|e| {
                        ShardError {
                            status: 500,
                            // ALLOC: error path; one message.
                            failure: Failure::opensearch("exception", e.to_string()),
                        }
                    })?;
                    // ALLOC: one field name a keyword source, bounded by the request's.
                    ords.insert(source.field.clone(), built);
                }
                CompositeValues::Keyword
            }
            (Reads::Long, SourceKind::Terms) => CompositeValues::Long,
            (Reads::Long, SourceKind::DateHistogram { rounding }) => {
                CompositeValues::Date(rounding)
            }
            (Reads::Long, SourceKind::Histogram { interval }) => CompositeValues::Histogram {
                kind: ValueKind::Long,
                interval,
            },
        };
        sources.push(CompositeSource {
            // ALLOC: one field name a source, bounded by the request's.
            field: source.field.clone(),
            values,
            reverse: source.desc,
            missing_bucket: source.missing_bucket,
            missing_order: match source.missing_order {
                MissingOrder::First => EngineMissing::First,
                MissingOrder::Last => EngineMissing::Last,
                MissingOrder::Default => EngineMissing::Default,
            },
        });
        kinds.push(kind);
    }
    Ok(AggNode::Composite {
        sources,
        size: spec.size,
        after: after_of(&spec, &kinds)?,
        // ALLOC: one flag a segment.
        sorted_lead: vec![false; segments],
        // ALLOC: empty; sub-aggregations are refused.
        subs: Vec::new(),
    })
}

/// The result of no bucket: a source on an unmapped field, or no match.
pub fn empty(agg: &MetricAgg) -> Value {
    // ALLOC: one box a `composite`, bounded by the request's.
    Value::Composite(Box::new(CompositeResult {
        spec: spec_of(agg),
        // ALLOC: empty.
        buckets: Vec::new(),
    }))
}

/// The shard-level result of `agg` from the engine's `result`.
pub fn value_from(agg: &MetricAgg, result: Option<AggResult>) -> Result<Value, ShardError> {
    let Some(AggResult::Composite { buckets, .. }) = result else {
        return Err(ShardError {
            status: 500,
            failure: Failure::opensearch(
                "exception",
                // ALLOC: error path; one message.
                "an aggregation result of another kind".to_owned(),
            ),
        });
    };
    // ALLOC: one entry a bucket, at most `size`, which the shard bounds.
    let kept = buckets
        .into_iter()
        .map(|(key, count)| {
            (
                key.into_iter()
                    .map(|v| match v {
                        CompositeKeyValue::Missing => KeyPart::Missing,
                        CompositeKeyValue::Long(v) => KeyPart::Long(v),
                        CompositeKeyValue::Double(v) => KeyPart::Double(v),
                        CompositeKeyValue::Term(t) => KeyPart::Term(t),
                    })
                    // ALLOC: one part a source, bounded by the request's.
                    .collect(),
                count,
            )
        })
        // ALLOC: one entry a bucket, at most `size`, as above.
        .collect();
    // ALLOC: one box a `composite`, bounded by the request's.
    Ok(Value::Composite(Box::new(CompositeResult {
        spec: spec_of(agg),
        buckets: kept,
    })))
}

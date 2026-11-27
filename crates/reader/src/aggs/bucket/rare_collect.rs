//! Running a `rare_terms` on one shard: the engine's `SignificantTerms` node
//! (`terms: true`), which counts every term of the field, filtered to the
//! terms held by at most `max_doc_count` documents.

use std::collections::HashMap;
use std::sync::Arc;

use lucene_aggs::bucket_aggs::{AggNode, AggResult, SignificantKey};
use lucene_search::global_ords::GlobalOrds;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::bucket::keyword_collect::{self, Parent};
use crate::aggs::bucket::rare::{RareKey, RareResult, RareSpec};
use crate::aggs::bucket::terms::TermsSpec;
use crate::aggs::bucket::terms_collect::too_many_buckets;
use crate::aggs::metric::MetricAgg;
use crate::aggs::partial::Value;
use crate::failure::Failure;
use crate::mapping::FieldType;
use crate::search::ShardError;

/// The spec of `agg`, which is a `rare_terms`.
pub fn spec_of(agg: &MetricAgg) -> RareSpec {
    agg.rare.as_deref().cloned().unwrap_or_default()
}

fn engine_error(reason: &str) -> ShardError {
    ShardError {
        status: 500,
        // ALLOC: error path; one message.
        failure: Failure::opensearch("exception", reason.to_owned()),
    }
}

/// The result of a field this shard does not map: no terms at all.
pub fn unmapped(spec: &RareSpec) -> Value {
    // ALLOC: one box a `rare_terms`, bounded by the request's.
    Value::Rare(Box::new(RareResult {
        // ALLOC: one spec, as above.
        spec: spec.clone(),
        ..RareResult::default()
    }))
}

/// Whether `agg`'s field is a number on `shard`.
fn is_numeric<S: BlobStore + 'static>(agg: &MetricAgg, shard: &OpenShard<S>) -> bool {
    shard
        .meta
        .mapping
        .get(&agg.field)
        .is_some_and(|f| f.ty != FieldType::Keyword)
}

/// The engine's node for `agg`, building the field's ordinals into `ords`.
pub fn node<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    ords: &mut HashMap<String, Arc<GlobalOrds>>,
) -> Result<AggNode, ShardError> {
    if is_numeric(agg, shard) {
        return Ok(AggNode::SignificantTerms {
            // ALLOC: one field name an aggregation, bounded by the request's.
            field: agg.field.clone(),
            numeric: true,
            terms: false,
            missing_term: None,
            missing: None,
            text: None,
            // ALLOC: empty; sub-aggregations are refused.
            subs: Vec::new(),
        });
    }
    if !ords.contains_key(&agg.field) {
        let built = shard
            .reader
            .global_ords(&agg.field)
            // ALLOC: error path; one message.
            .map_err(|e| engine_error(&e.to_string()))?;
        // ALLOC: one field name a keyword field, bounded by the request's.
        ords.insert(agg.field.clone(), built);
    }
    Ok(AggNode::SignificantTerms {
        // ALLOC: one field name an aggregation, bounded by the request's.
        field: agg.field.clone(),
        numeric: false,
        terms: true,
        missing_term: None,
        missing: None,
        text: None,
        // ALLOC: empty; sub-aggregations are refused.
        subs: Vec::new(),
    })
}

/// The most bytes `agg` holds on `shard`: the field's ordinals and the
/// engine's candidates, as a keyword `terms` keeping the bucket limit holds
/// them.
pub fn reserved<S: BlobStore + 'static>(shard: &OpenShard<S>, agg: &MetricAgg) -> usize {
    if is_numeric(agg, shard) {
        // The engine holds a state for each distinct value, at most one a
        // document.
        let docs = usize::try_from(shard.reader.max_doc()).unwrap_or(usize::MAX);
        return docs.saturating_mul(std::mem::size_of::<(i64, u64)>().saturating_mul(4));
    }
    let terms = TermsSpec {
        size: BUCKET_LIMIT,
        shard_size: BUCKET_LIMIT,
        ..TermsSpec::default()
    };
    keyword_collect::reserved_bytes(shard, &agg.field, &terms, 0, Parent::TOP)
}

/// The shard-level result of `agg` from the engine's `result`: the terms
/// held by at most `max_doc_count` documents with their counts, and the
/// others as excluded.
pub fn value<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    result: Option<AggResult>,
) -> Result<Value, ShardError> {
    let long = is_numeric(agg, shard);
    let spec = spec_of(agg);
    let keys = match result {
        Some(AggResult::Significant { buckets, .. }) => {
            buckets.into_iter().next().map(|b| b.1).unwrap_or_default()
        }
        // ALLOC: empty.
        None => Vec::new(),
        Some(AggResult::Refused(why)) => return Err(engine_error(&why)),
        Some(_) => return Err(engine_error("an aggregation result of another kind")),
    };
    if keys.len() > BUCKET_LIMIT {
        return Err(too_many_buckets(keys.len(), BUCKET_LIMIT));
    }
    // ALLOC: one entry a term of the field, at most the bucket limit.
    let mut buckets = Vec::new();
    // ALLOC: as above.
    let mut excluded = Vec::new();
    for (key, count) in keys {
        let term = match key {
            SignificantKey::Term(term) => {
                // ALLOC: one term a candidate, as above.
                RareKey::Text(String::from_utf8_lossy(&term).into_owned())
            }
            SignificantKey::Long(n) => RareKey::Long(n),
        };
        if count <= spec.max_doc_count {
            buckets.push((term, count));
        } else {
            excluded.push(term);
        }
    }
    buckets.sort_by(|a, b| a.0.cmp(&b.0));
    // ALLOC: one box a `rare_terms`, bounded by the request's.
    Ok(Value::Rare(Box::new(RareResult {
        spec,
        long,
        buckets,
        excluded,
    })))
}

/// The most bytes every `rare_terms` of `aggs` holds on `shard`.
pub fn reserved_all<S: BlobStore + 'static>(shard: &OpenShard<S>, aggs: &[MetricAgg]) -> usize {
    aggs.iter()
        .filter(|agg| agg.metric == crate::aggs::metric::Metric::RareTerms)
        .fold(0usize, |total, agg| {
            total.saturating_add(reserved(shard, agg))
        })
}

//! Running a `significant_terms` on one shard: the engine's
//! `SignificantTerms` node (`MapStringTermsAggregator`) for each term's
//! foreground count and the matches' number, and the background counts
//! (`SignificanceLookup`) read from the segments' dictionaries.

use std::collections::HashMap;
use std::sync::Arc;

use lucene_aggs::bucket_aggs::{AggNode, AggResult, SignificantKey};
use lucene_search::global_ords::GlobalOrds;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::keyword_collect::{self, Parent};
use crate::aggs::bucket::significant::{SigBucket, SignificantResult, SignificantSpec};
use crate::aggs::bucket::terms::TermsSpec;
use crate::aggs::metric::MetricAgg;
use crate::aggs::partial::Value;
use crate::failure::Failure;
use crate::search::ShardError;

/// The spec of `agg`, which is a `significant_terms`.
pub fn spec_of(agg: &MetricAgg) -> SignificantSpec {
    agg.significant.as_deref().cloned().unwrap_or_default()
}

fn engine_error(reason: &str) -> ShardError {
    ShardError {
        status: 500,
        // ALLOC: error path; one message.
        failure: Failure::opensearch("exception", reason.to_owned()),
    }
}

/// The result of a field this shard does not map: no documents at all.
pub fn unmapped(spec: &SignificantSpec) -> Value {
    // ALLOC: one box a `significant_terms`, bounded by the request's.
    Value::Significant(Box::new(SignificantResult {
        // ALLOC: one spec, as above.
        spec: spec.clone(),
        subset_size: 0,
        superset_size: 0,
        // ALLOC: empty.
        buckets: Vec::new(),
    }))
}

/// The engine's node for `agg`, building the field's ordinals into `ords`.
pub fn node<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    ords: &mut HashMap<String, Arc<GlobalOrds>>,
) -> Result<AggNode, ShardError> {
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
/// engine's candidates, as a keyword `terms` of the same size holds them,
/// and one key and two counts a kept bucket.
pub fn reserved<S: BlobStore + 'static>(shard: &OpenShard<S>, agg: &MetricAgg) -> usize {
    let spec = spec_of(agg);
    let terms = TermsSpec {
        size: spec.size,
        shard_size: spec.size,
        ..TermsSpec::default()
    };
    keyword_collect::reserved_bytes(shard, &agg.field, &terms, 0, Parent::TOP)
}

/// The shard-level result of `agg` from the engine's `result`: each term
/// with at least `min_doc_count` matches gets its background count, and the
/// best `size` by score are kept.
pub fn value<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    result: Option<AggResult>,
) -> Result<Value, ShardError> {
    let spec = spec_of(agg);
    let (subset_size, keys) = match result {
        Some(AggResult::Significant { buckets, .. }) => {
            buckets.into_iter().next().unwrap_or_default()
        }
        // ALLOC: empty.
        None => (0, Vec::new()),
        Some(AggResult::Refused(why)) => return Err(engine_error(&why)),
        Some(_) => return Err(engine_error("an aggregation result of another kind")),
    };
    let superset_size = u64::try_from(shard.reader.max_doc()).unwrap_or(0);
    // ALLOC: one bucket a candidate term, bounded by the field's terms.
    let mut buckets = Vec::new();
    for (key, count) in keys {
        if count < spec.min_doc_count {
            continue;
        }
        let SignificantKey::Term(term) = key else {
            return Err(engine_error("a significant_terms key that is not a term"));
        };
        let mut superset_df = 0u64;
        for segment in shard.reader.segment_readers() {
            let df = segment
                .doc_freq(&agg.field, &term)
                // ALLOC: error path; one message.
                .map_err(|e| engine_error(&e.to_string()))?;
            superset_df = superset_df.saturating_add(u64::try_from(df).unwrap_or(0));
        }
        buckets.push(SigBucket {
            // ALLOC: one term a candidate, as above.
            term: String::from_utf8_lossy(&term).into_owned(),
            subset_df: count,
            superset_df,
        });
    }
    let mut result = SignificantResult {
        spec,
        subset_size,
        superset_size,
        buckets,
    };
    result.select();
    // ALLOC: one box a `significant_terms`, bounded by the request's.
    Ok(Value::Significant(Box::new(result)))
}

/// The most bytes every `significant_terms` of `aggs` holds on `shard`.
pub fn reserved_all<S: BlobStore + 'static>(shard: &OpenShard<S>, aggs: &[MetricAgg]) -> usize {
    aggs.iter()
        .filter(|agg| agg.metric == crate::aggs::metric::Metric::SignificantTerms)
        .fold(0usize, |total, agg| {
            total.saturating_add(reserved(shard, agg))
        })
}

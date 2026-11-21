//! `cardinality` of a keyword field on one shard: the engine's node over the
//! field's global ordinals (`CardinalityAggregator.OrdinalsCollector`, which
//! hashes each distinct term seen), and the memory it holds.

use std::collections::HashMap;
use std::sync::Arc;

use lucene_aggs::bucket_aggs::{AggNode, CardinalityKind};
use lucene_search::global_ords::GlobalOrds;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::keyword_collect::{self, ORD_BYTES};
use crate::aggs::metric::{Metric, MetricAgg};
use crate::failure::Failure;
use crate::search::ShardError;

/// The engine's node for the keyword `cardinality` `agg`, building the
/// ordinals of its field into `ords`.
pub fn node<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    ords: &mut HashMap<String, Arc<GlobalOrds>>,
) -> Result<AggNode, ShardError> {
    let precision = agg.cardinality.precision().map_err(|m| ShardError {
        status: 400,
        failure: Failure::plain("illegal_argument_exception", m),
    })?;
    if !ords.contains_key(&agg.field) {
        let built = shard
            .reader
            .global_ords(&agg.field)
            .map_err(|e| ShardError {
                status: 500,
                // ALLOC: error path; one message.
                failure: Failure::opensearch("exception", e.to_string()),
            })?;
        // ALLOC: one field name an aggregation, bounded by the request's.
        ords.insert(agg.field.clone(), built);
    }
    Ok(AggNode::Cardinality {
        // ALLOC: one field name an aggregation, bounded by the request's.
        field: agg.field.clone(),
        kind: CardinalityKind::Keyword,
        precision: Some(precision),
    })
}

/// The most bytes the keyword `cardinality`s among `aggs` hold on `shard`
/// beyond their sketches (which `state_bytes` counts): the global ordinals of each distinct field
/// ([`ORD_BYTES`] a term), the bit a term of the one owning bucket, and the
/// term buffers read while hashing.
pub fn reserved<S: BlobStore + 'static>(aggs: &[MetricAgg], shard: &OpenShard<S>) -> usize {
    // ALLOC: one name a distinct field, bounded by the request's.
    let mut seen: Vec<&str> = Vec::new();
    let mut total = 0usize;
    for agg in aggs.iter().filter(|a| a.metric == Metric::Cardinality) {
        let (terms, segments, longest) = keyword_collect::dictionary(shard, &agg.field);
        let bits = terms.div_ceil(8);
        let own = if seen.contains(&agg.field.as_str()) {
            0
        } else {
            // ALLOC: as above.
            seen.push(agg.field.as_str());
            terms
                .saturating_mul(ORD_BYTES)
                .saturating_add(segments.saturating_mul(longest).saturating_mul(2))
        };
        total = total.saturating_add(own).saturating_add(bits);
    }
    total
}

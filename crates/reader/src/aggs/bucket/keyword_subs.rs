//! The sub-aggregations of a keyword `terms` on one shard: their engine
//! nodes under the `terms` node, their per-bucket values from the engine's
//! result, and the memory they hold. A sub-aggregation is a single-value or
//! stats metric, or a keyword `terms` with sub-aggregations of its own.

use std::collections::HashMap;

use lucene_aggs::bucket_aggs::{AggNode, AggResult};
use lucene_aggs::metric_aggs::MetricState;
use lucene_search::global_ords::GlobalOrds;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::bucket::keyword_collect::{self, Parent};
use crate::aggs::bucket::keyword_terms::KeywordTermsResult;
use crate::aggs::bucket::terms::TermsSpec;
use crate::aggs::metric::collect::{Resolved, node as metric_node, resolve, value_of};
use crate::aggs::metric::{DEFAULT_PRECISION, Metric, MetricAgg};
use crate::aggs::partial::{Format, Value};
use crate::failure::Failure;
use crate::search::ShardError;

/// `agg`'s `terms` options (every `terms` has them).
fn spec_of(agg: &MetricAgg) -> TermsSpec {
    agg.terms.as_deref().cloned().unwrap_or_default()
}

/// Resolves each sub-aggregation of `agg` as a top-level one is resolved, so
/// an unreadable field fails the shard. A numeric `terms` is refused.
fn resolved<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
) -> Result<Vec<Resolved>, ShardError> {
    // ALLOC: one entry a sub-aggregation, bounded by the request's.
    let mut out = Vec::with_capacity(agg.subs.len());
    for sub in &agg.subs {
        let r = resolve(sub, shard)?;
        if sub.metric == Metric::Terms && matches!(r, Resolved::Numeric(_)) {
            return Err(ShardError {
                status: 400,
                failure: Failure::plain(
                    "illegal_argument_exception",
                    // ALLOC: error path; one message.
                    format!(
                        "a [terms] sub-aggregation of the numeric field [{}] is not supported by \
                         this node yet",
                        sub.field
                    ),
                ),
            });
        }
        out.push(r);
    }
    Ok(out)
}

fn engine_error(reason: &str) -> ShardError {
    ShardError {
        status: 500,
        // ALLOC: error path; one message.
        failure: Failure::opensearch("exception", reason.to_owned()),
    }
}

/// The engine's node for the keyword `terms` `agg` with its
/// sub-aggregations beneath, building the ordinals of every keyword field it
/// reads into `ords`. An unmapped field's sub-aggregation has no node, and
/// its buckets read the empty result.
pub fn node<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    ords: &mut HashMap<String, std::sync::Arc<GlobalOrds>>,
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
    let spec = spec_of(agg);
    let accepted = match ords.get(&agg.field) {
        Some(g) => keyword_collect::accepted_ords(shard, &agg.field, g, &spec)?,
        None => None,
    };
    let mut engine = keyword_collect::node(&agg.field, &spec, accepted);
    if let AggNode::Terms { subs, .. } = &mut engine {
        for (sub, r) in agg.subs.iter().zip(resolved(agg, shard)?) {
            match r {
                Resolved::Numeric(_) => subs.push(metric_node(sub, DEFAULT_PRECISION)),
                Resolved::Keyword => subs.push(node(sub, shard, ords)?),
                Resolved::Unmapped => {}
            }
        }
    }
    Ok(engine)
}

/// The most bytes the keyword `terms` `agg` holds on `shard` counting under
/// `parent`, with every sub-aggregation: its own
/// ([`keyword_collect::reserved_bytes`]), and a nested `terms`' under this
/// one's cells and kept buckets.
pub fn reserved<S: BlobStore + 'static>(
    shard: &OpenShard<S>,
    agg: &MetricAgg,
    parent: Parent,
) -> usize {
    let spec = spec_of(agg);
    let nested = agg.subs.iter().filter(|s| s.metric == Metric::Terms);
    let scalars = agg.subs.len().saturating_sub(nested.clone().count());
    let own = keyword_collect::reserved_bytes(shard, &agg.field, &spec, scalars, parent);
    let (terms, _, _) = keyword_collect::dictionary(shard, &agg.field);
    let under = Parent {
        cells: parent.cells(terms),
        kept: parent
            .kept
            .saturating_mul(spec.shard_size.min(terms))
            .min(crate::aggs::bucket::BUCKET_LIMIT),
    };
    nested.fold(own, |total, sub| {
        total.saturating_add(reserved(shard, sub, under))
    })
}

/// One value an owning bucket for `agg` from the engine's `result`, which
/// holds `owners` owning buckets: the keyword `terms` of each, or the
/// metric of each. An unmapped field has the empty value in each.
fn build<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    result: Option<AggResult>,
    owners: usize,
) -> Result<Vec<Value>, ShardError> {
    if agg.metric != Metric::Terms {
        let states = match result {
            Some(AggResult::Metric(states)) => states,
            // ALLOC: empty.
            None => Vec::new(),
            Some(_) => return Err(engine_error("a sub-aggregation result of another kind")),
        };
        let empty = MetricState::default();
        // ALLOC: one value an owning bucket, bounded by the kept buckets.
        return Ok((0..owners)
            .map(|at| value_of(agg, states.get(at).unwrap_or(&empty)))
            .collect());
    }
    let spec = spec_of(agg);
    let (mut buckets, subs) = match result {
        Some(AggResult::Terms { buckets, subs }) => (buckets, subs),
        // ALLOC: empty.
        None => (Vec::new(), Vec::new()),
        Some(_) => return Err(engine_error("an aggregation result of another kind")),
    };
    buckets.resize_with(owners, Default::default);
    // ALLOC: one result an owning bucket, bounded by the kept buckets.
    let mut results: Vec<KeywordTermsResult> = buckets
        .into_iter()
        .map(|(other, kept)| keyword_collect::from_engine(&spec, other, kept))
        // ALLOC: as above.
        .collect();
    let total: usize = results.iter().map(|r| r.buckets.len()).sum();
    let mut engine = subs.into_iter();
    // ALLOC: one list a kept bucket, bounded as above.
    let mut values: Vec<Vec<Value>> = vec![Vec::new(); total];
    for (sub, r) in agg.subs.iter().zip(resolved(agg, shard)?) {
        let (format, result) = match r {
            Resolved::Numeric(format) => (format, engine.next()),
            Resolved::Keyword => (Format::Raw, engine.next()),
            Resolved::Unmapped => (Format::Raw, None),
        };
        let made = build(sub, shard, result, total)?;
        for (list, value) in values.iter_mut().zip(made) {
            list.push(value);
        }
        for r in &mut results {
            // ALLOC: one entry a sub-aggregation, bounded by the request's.
            r.sub_meta.push((sub.name.clone(), format));
        }
    }
    let mut lists = values.into_iter();
    for r in &mut results {
        // ALLOC: one list a kept bucket, as above.
        r.subs = lists.by_ref().take(r.buckets.len()).collect();
    }
    // ALLOC: one box an owning bucket, bounded as above.
    Ok(results
        .into_iter()
        .map(|r| Value::KeywordTerms(Box::new(r)))
        // ALLOC: one box an owning bucket, as above.
        .collect())
}

/// The shard-level result of the keyword `terms` `agg` from the engine's
/// `result`, with every kept bucket's sub-aggregation values.
pub fn value<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    result: Option<AggResult>,
) -> Result<Value, ShardError> {
    if agg.subs.is_empty() {
        return keyword_collect::value(&spec_of(agg), result);
    }
    build(agg, shard, result, 1)?
        .into_iter()
        .next()
        .ok_or_else(|| engine_error("an aggregation result of another kind"))
}

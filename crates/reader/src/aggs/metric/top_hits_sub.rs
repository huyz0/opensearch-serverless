//! A `top_hits` under a bucket aggregation on one shard: the engine's node
//! that keeps each owning bucket's hits, and the bucket's result from them.

use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::metric::top_hits::{TopHitsPlan, TopHitsResult, plan};
use crate::aggs::metric::{MetricAgg, Value};
use crate::failure::Failure;
use crate::fetch::docvalue_fields;
use crate::search::{Hit, ShardError};
use crate::search_sort::KeyKind;

/// The engine's node for a `top_hits` under a bucket aggregation: each
/// owning bucket's collector of the window, by score or in the sort.
///
/// # Errors
///
/// The refusals of [`plan`], and of an `explain` or a `highlight`, which a
/// bucket's hits do not carry yet.
pub(crate) fn sub_node<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
) -> Result<lucene_aggs::bucket_aggs::AggNode, ShardError> {
    use lucene_aggs::bucket_aggs::AggNode;
    let plans = sub_plan(agg, shard)?;
    let Some(plan) = plans.into_iter().next() else {
        return Err(ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            "a sub-aggregation holds no top_hits",
        )));
    };
    Ok(match plan.sort {
        None => AggNode::TopHits { top_n: plan.window },
        Some((fields, _)) => AggNode::SortedTopHits {
            top_n: plan.window,
            sort: std::sync::Arc::new(fields),
            track_scores: false,
        },
    })
}

fn sub_plan<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
) -> Result<Vec<TopHitsPlan>, ShardError> {
    if let Some(spec) = agg.top_hits.as_deref()
        && (spec.explain || spec.highlight.is_some())
    {
        return Err(ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "[explain] and [highlight] of the [top_hits] aggregation [{}] under a bucket \
                 aggregation are not supported by this node yet",
                agg.name
            ),
        )));
    }
    // The index's uuid stands for its name in a sort's refusal.
    plan(std::slice::from_ref(agg), shard, &shard.meta.uuid)
}

/// Whether a `top_hits` among `aggs` or beneath them sorts on a numeric
/// field, which needs the points open.
pub(crate) fn any_numeric_sub<S: BlobStore + 'static>(
    aggs: &[MetricAgg],
    shard: &OpenShard<S>,
) -> bool {
    aggs.iter().any(|agg| {
        agg.subs.iter().any(|sub| {
            (sub.top_hits.is_some()
                && sub_plan(sub, shard)
                    .map(|p| p.iter().any(TopHitsPlan::numeric))
                    .unwrap_or(false))
                || any_numeric_sub(std::slice::from_ref(sub), shard)
        })
    })
}

/// Each owning bucket's `top_hits` result for `agg` from the engine's
/// `result` (none for a shard that collected nothing): the hits after
/// `from`, and those ahead of it for a coordinator's merge.
///
/// # Errors
///
/// A result of another kind, or a `docvalue_fields` the hits cannot take.
pub(crate) fn sub_values<S: BlobStore + 'static>(
    agg: &MetricAgg,
    shard: &OpenShard<S>,
    result: Option<lucene_aggs::bucket_aggs::AggResult>,
    owners: usize,
) -> Result<Vec<Value>, ShardError> {
    use lucene_aggs::bucket_aggs::AggResult;
    let Some(spec) = agg.top_hits.as_deref() else {
        return Err(ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            "a sub-aggregation holds no top_hits",
        )));
    };
    let kinds: Vec<KeyKind> = match sub_plan(agg, shard)?.into_iter().next() {
        Some(TopHitsPlan {
            sort: Some((_, kinds)),
            ..
        }) => kinds,
        // ALLOC: empty.
        _ => Vec::new(),
    };
    // One (total, hits) a bucket that collected something.
    let found: Vec<Option<(u64, Vec<Hit>)>> = match result {
        // ALLOC: empty.
        None => Vec::new(),
        Some(AggResult::TopHits(per)) => per
            .into_iter()
            .map(|b| {
                b.map(|(total, docs)| {
                    (
                        total,
                        docs.into_iter()
                            .map(|d| Hit {
                                doc: d.doc_id,
                                score: Some(d.score),
                                // ALLOC: empty.
                                sort: Vec::new(),
                            })
                            // ALLOC: one hit, at most the inner result window a bucket.
                            .collect(),
                    )
                })
            })
            // ALLOC: one entry an owning bucket.
            .collect(),
        Some(AggResult::SortedTopHits(per)) => per
            .into_iter()
            .map(|b| {
                b.map(|s| {
                    let hits = s
                        .top
                        .hits
                        .iter()
                        .map(|h| {
                            let mut hit = crate::search_top::sorted_hit(h, &kinds, None);
                            hit.score = None;
                            hit
                        })
                        // ALLOC: one hit, at most the inner result window a bucket.
                        .collect();
                    (s.top.total.value, hits)
                })
            })
            // ALLOC: one entry an owning bucket.
            .collect(),
        Some(_) => {
            return Err(ShardError {
                status: 500,
                failure: Failure::opensearch(
                    "exception",
                    // ALLOC: error path; one message.
                    "a top_hits result of another kind".to_owned(),
                ),
            });
        }
    };
    let fetches = found
        .iter()
        .flatten()
        .any(|(_, hits)| hits.len() > spec.from);
    let fields = if spec.docvalue_fields.is_empty() {
        // ALLOC: empty.
        Ok(Vec::new())
    } else {
        docvalue_fields::resolve(&spec.docvalue_fields, &shard.meta.mapping)
    };
    let fields = match fields {
        Ok(fields) => fields,
        Err(e) if fetches => return Err(e),
        // ALLOC: empty.
        Err(_) => Vec::new(),
    };
    // ALLOC: one value an owning bucket.
    let mut out = Vec::with_capacity(owners);
    for at in 0..owners {
        let mut result = TopHitsResult::empty();
        if let Some(Some((total, mut before))) = found.get(at).cloned() {
            let hits = before.split_off(spec.from.min(before.len()));
            result.total = total;
            result.max_score = if kinds.is_empty() {
                before.first().or(hits.first()).and_then(|h| h.score)
            } else {
                None
            };
            result.before = before;
            result.hits = hits;
            result.source = spec.source.clone();
            result.fields = fields.clone();
        }
        // ALLOC: one box a `top_hits` a bucket, bounded by the buckets.
        out.push(Value::TopHits(Box::new(result)));
    }
    Ok(out)
}

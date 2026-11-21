//! Running a `filters` on one shard: each filter built as the query it is
//! (`FiltersAggregatorFactory`), the engine's `filters` node
//! (`FiltersAggregator`) and its result as `InternalFilters` holds it.

use lucene_aggs::bucket_aggs::AggNode;
use lucene_search::BooleanQuery;

use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::bucket::filters::{FiltersResult, FiltersSpec};
use crate::aggs::bucket::terms_collect::too_many_buckets;
use crate::aggs::metric::MetricAgg;
use crate::aggs::partial::Value;
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, to_lucene};
use crate::search::ShardError;

/// `agg`'s `filters` options (every `filters` has them).
fn spec_of(agg: &MetricAgg) -> FiltersSpec {
    agg.filters.as_deref().cloned().unwrap_or_default()
}

/// The queries of each aggregation's filters, one list an aggregation in
/// request order: those of the aggregation itself, then of every `filters`
/// and `filter` beneath it, depth first in request order (see [`Queries`]).
pub fn plan(aggs: &[MetricAgg], cx: &QueryContext<'_>) -> Result<Vec<Vec<BooleanQuery>>, Failure> {
    // ALLOC: one list an aggregation, bounded by the request's.
    let mut plans = Vec::with_capacity(aggs.len());
    for agg in aggs {
        // ALLOC: one query a filter, bounded by the body's recording.
        let mut queries = Vec::new();
        collect_queries(agg, cx, &mut queries)?;
        plans.push(queries);
    }
    Ok(plans)
}

fn collect_queries(
    agg: &MetricAgg,
    cx: &QueryContext<'_>,
    out: &mut Vec<BooleanQuery>,
) -> Result<(), Failure> {
    if let Some(spec) = agg.filters.as_deref() {
        for (_, filter) in &spec.filters {
            out.push(to_lucene(filter, cx)?);
        }
    }
    for sub in &agg.subs {
        collect_queries(sub, cx, out)?;
    }
    Ok(())
}

/// How many queries the filters of `agg` and its sub-aggregations hold.
fn tree_queries(agg: &MetricAgg) -> usize {
    agg.subs.iter().fold(own_queries(agg), |n, sub| {
        n.saturating_add(tree_queries(sub))
    })
}

fn own_queries(agg: &MetricAgg) -> usize {
    agg.filters.as_deref().map_or(0, |spec| spec.filters.len())
}

/// The compiled queries of one top-level aggregation's tree ([`plan`]'s
/// list), taken in the order the engine's nodes are built.
#[derive(Debug)]
pub struct Queries<'a> {
    rest: &'a [BooleanQuery],
}

impl<'a> Queries<'a> {
    /// The queries `plan` made for one aggregation (none for an unplanned one).
    pub fn new(list: Option<&'a Vec<BooleanQuery>>) -> Self {
        Self {
            rest: list.map_or(&[], Vec::as_slice),
        }
    }

    /// The queries of `agg`'s own filters.
    pub fn take_own(&mut self, agg: &MetricAgg) -> &'a [BooleanQuery] {
        self.take(own_queries(agg))
    }

    /// Passes the queries of `agg`'s whole tree (a sub-aggregation with no node).
    pub fn skip_tree(&mut self, agg: &MetricAgg) {
        self.take(tree_queries(agg));
    }

    fn take(&mut self, n: usize) -> &'a [BooleanQuery] {
        let (taken, rest) = self.rest.split_at(n.min(self.rest.len()));
        self.rest = rest;
        taken
    }
}

/// The engine's node for a `filters` of `queries`, one a filter.
pub fn node(agg: &MetricAgg, queries: &[BooleanQuery]) -> AggNode {
    AggNode::Filters {
        // ALLOC: one query a filter, bounded by the body's recording.
        filters: queries.to_vec(),
        other: spec_of(agg).other_key.is_some(),
        // ALLOC: empty; sub-aggregations are refused.
        subs: Vec::new(),
    }
}

/// How many buckets `agg` makes: a filter each and the other one.
pub fn bucket_count(spec: &FiltersSpec) -> usize {
    spec.filters
        .len()
        .saturating_add(usize::from(spec.other_key.is_some()))
}

/// What a `filters` holds beyond its engine state: a bucket and key a
/// filter and the other, and the engine's count; the queries are bounded
/// by the request's body.
pub fn state_bytes(spec: &FiltersSpec) -> usize {
    let keys = spec.filters.iter().fold(0usize, |n, (key, _)| {
        n.saturating_add(key.len().saturating_mul(2))
    });
    let other = spec
        .other_key
        .as_ref()
        .map_or(0, |key| key.len().saturating_mul(2));
    bucket_count(spec)
        .min(BUCKET_LIMIT)
        .saturating_mul(std::mem::size_of::<(String, u64)>().saturating_add(8))
        .saturating_add(keys)
        .saturating_add(other)
}

/// More buckets than the bucket limit fail the shard.
pub fn check_limit(agg: &MetricAgg) -> Result<(), ShardError> {
    match agg.filters.as_deref().map(bucket_count) {
        Some(made) if made > BUCKET_LIMIT => Err(too_many_buckets(made, BUCKET_LIMIT)),
        _ => Ok(()),
    }
}

/// `agg`'s result with the count of bucket `i` taken from `docs[i]`: the
/// engine's result, or no document at all.
pub fn value(agg: &MetricAgg, docs: &[u64]) -> Value {
    let spec = spec_of(agg);
    let other = spec.other_key.iter();
    // ALLOC: one bucket a filter and the other, bounded by the request's.
    let buckets = spec
        .filters
        .iter()
        .map(|(key, _)| key)
        .chain(other)
        .enumerate()
        // ALLOC: one key a bucket, bounded by the request's.
        .map(|(i, key)| (key.clone(), docs.get(i).copied().unwrap_or(0)))
        .collect();
    // ALLOC: one box a `filters`, bounded by the request's.
    Value::Filters(Box::new(FiltersResult {
        single: spec.single,
        keyed: spec.keyed,
        buckets,
        ..FiltersResult::default()
    }))
}

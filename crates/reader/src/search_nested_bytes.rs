//! What a search reserves for the parent bit sets of its nested joins.
//!
//! Each join node (a `nested` query, its `inner_hits`, a `nested` or
//! `reverse_nested` aggregation, a nested sort key) builds its own
//! `QueryBitSetProducer`, which marks the root documents of a segment in one
//! bit a Lucene document, lazily and cached for the search.

use oss_blob::BlobStore;

use crate::OpenShard;
use crate::aggs::metric::{Metric, MetricAgg};
use crate::dsl::SearchSource;
use crate::dsl::queries::More;
use crate::dsl::query::Query;
use crate::dsl::sort::SortKey;

/// The nested joins `q` holds.
fn query_joins(q: &Query) -> usize {
    match q {
        Query::Bool(b) => b
            .must
            .iter()
            .chain(&b.should)
            .chain(&b.must_not)
            .chain(&b.filter)
            .map(query_joins)
            .sum(),
        Query::More(more) => match &**more {
            More::ConstantScore { filter, .. } => query_joins(filter),
            More::DisMax { queries, .. } => queries.iter().map(query_joins).sum(),
            More::FunctionScore(f, _) => f
                .query
                .iter()
                .chain(f.functions.iter().filter_map(|e| e.filter.as_ref()))
                .map(query_joins)
                .sum(),
            More::Nested(n, _) => 1usize
                .saturating_add(usize::from(n.inner_hits.is_some()))
                .saturating_add(query_joins(&n.query)),
            More::Knn(k) => k.filter.as_ref().map_or(0, query_joins),
            _ => 0,
        },
        _ => 0,
    }
}

/// The nested joins `agg` and its sub-aggregations hold.
fn agg_joins(agg: &MetricAgg) -> usize {
    let own = usize::from(matches!(agg.metric, Metric::Nested | Metric::ReverseNested));
    let filters = agg.filters.as_deref().map_or(0, |spec| {
        spec.filters.iter().map(|(_, q)| query_joins(q)).sum()
    });
    agg.subs
        .iter()
        .map(agg_joins)
        .fold(own.saturating_add(filters), usize::saturating_add)
}

/// The nested joins `source` holds.
pub(crate) fn joins(source: &SearchSource) -> usize {
    let sorts = source.sort.iter().map(|key| match key {
        SortKey::Field {
            nested: Some(n), ..
        } => 1usize.saturating_add(n.filter.as_ref().map_or(0, query_joins)),
        _ => 0,
    });
    source
        .query
        .iter()
        .chain(&source.post_filter)
        .map(query_joins)
        .chain(source.aggs.iter().map(agg_joins))
        .chain(sorts)
        .fold(0, usize::saturating_add)
}

/// The most bytes the parent bit sets of `source`'s joins hold on `shard`: a
/// bit a document and a word of slack a segment, for each join.
pub(crate) fn bytes<S: BlobStore + 'static>(source: &SearchSource, shard: &OpenShard<S>) -> usize {
    let docs = usize::try_from(shard.reader.max_doc()).unwrap_or(usize::MAX);
    let segments = shard.reader.segment_readers().len();
    let one = docs
        .div_ceil(8)
        .saturating_add(segments.saturating_mul(std::mem::size_of::<u64>()));
    joins(source).saturating_mul(one)
}

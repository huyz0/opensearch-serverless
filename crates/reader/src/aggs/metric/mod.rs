//! Metric aggregations on one shard: `min`, `max`, `sum`, `avg`, `stats`
//! and `value_count` of a numeric or date field, at the top level of a
//! search.
//!
//! A request's aggregations are parsed with its body ([`parse`]), computed
//! over the query's live matches in one pass of lucene-aggs, the engine's
//! port of OpenSearch's metric aggregators ([`collect()`]), and kept as the
//! shard-level results OpenSearch's `Internal*` aggregations hold
//! ([`partial`]): what a coordinator reduces across shards, with the same
//! reduce. A single-shard search answers them unreduced, as OpenSearch does
//! ([`render`]). Every other aggregation type, sub-aggregations, scripts,
//! `missing`, `format` and `meta` are refused as not served yet.

pub mod collect;
pub mod date_text;
pub mod double;
pub mod hash_order;
pub mod parse;
pub mod partial;
pub mod render;

pub use collect::collect;
pub use parse::parse_aggregations;
pub use partial::{Format, Partial, Value};
pub use render::write_aggregations;

/// Which metric an aggregation computes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    /// `min`: the smallest value.
    Min,
    /// `max`: the largest value.
    Max,
    /// `sum`: the values' compensated sum.
    Sum,
    /// `avg`: the sum over the count of values.
    Avg,
    /// `stats`: count, min, max, avg and sum.
    Stats,
    /// `value_count`: how many values.
    ValueCount,
}

impl Metric {
    /// Every metric served, by its aggregation type name.
    pub const ALL: [Metric; 6] = [
        Metric::Min,
        Metric::Max,
        Metric::Sum,
        Metric::Avg,
        Metric::Stats,
        Metric::ValueCount,
    ];

    /// The aggregation type name.
    pub fn name(self) -> &'static str {
        match self {
            Metric::Min => "min",
            Metric::Max => "max",
            Metric::Sum => "sum",
            Metric::Avg => "avg",
            Metric::Stats => "stats",
            Metric::ValueCount => "value_count",
        }
    }

    /// The metric of type `name`, if it is served.
    pub fn from_name(name: &str) -> Option<Metric> {
        Metric::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// One metric aggregation of a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetricAgg {
    /// The aggregation's name, its key in the response.
    pub name: String,
    /// What it computes.
    pub metric: Metric,
    /// The field it reads.
    pub field: String,
}

/// The most bytes aggregating `aggs` holds beyond the hits: each
/// aggregation's result with its name, and the engine's request and state
/// for it. What the caller reserves before searching.
pub fn state_bytes(aggs: &[MetricAgg]) -> usize {
    let per_agg = std::mem::size_of::<Partial>()
        .saturating_add(std::mem::size_of::<lucene_aggs::metric_aggs::MetricSpec>())
        .saturating_add(std::mem::size_of::<lucene_aggs::metric_aggs::MetricState>())
        .saturating_add(std::mem::size_of::<usize>());
    aggs.iter().fold(0usize, |total, agg| {
        total
            .saturating_add(per_agg)
            .saturating_add(agg.name.len())
            .saturating_add(agg.field.len())
    })
}

#[cfg(test)]
mod tests;

//! Metric aggregations on one shard: `min`, `max`, `sum`, `avg`, `stats`,
//! `value_count`, `cardinality`, `percentiles` and `percentile_ranks` of a
//! numeric or date field, and `top_hits`, at the top level of a search.
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
pub mod hll;
pub mod parse;
pub mod partial;
pub mod percentiles;
pub mod render;
pub mod tdigest;
pub mod top_hits;

pub use crate::aggs::partial::{Format, Partial, Value};
pub use collect::collect;
pub use parse::parse_aggregations;
pub use percentiles::PercentilesOptions;
pub use render::{write_aggregations, write_aggregations_with};
pub use top_hits::{TopHitsResult, TopHitsSpec};

use crate::aggs::bucket::terms::TermsSpec;

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
    /// `cardinality`: an estimate of how many distinct values, from a
    /// HyperLogLog++ sketch.
    Cardinality,
    /// `percentiles`: the values at given percents, from a t-digest.
    Percentiles,
    /// `percentile_ranks`: the percent of values at or below given values,
    /// from a t-digest.
    PercentileRanks,
    /// `top_hits`: the top documents in a sort of its own.
    TopHits,
    /// `terms`: the documents counted by each value of a numeric field (a
    /// bucket aggregation, held here as `top_hits` is).
    Terms,
}

impl Metric {
    /// Every metric served, by its aggregation type name.
    pub const ALL: [Metric; 11] = [
        Metric::Min,
        Metric::Max,
        Metric::Sum,
        Metric::Avg,
        Metric::Stats,
        Metric::ValueCount,
        Metric::Cardinality,
        Metric::Percentiles,
        Metric::PercentileRanks,
        Metric::TopHits,
        Metric::Terms,
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
            Metric::Cardinality => "cardinality",
            Metric::Percentiles => "percentiles",
            Metric::PercentileRanks => "percentile_ranks",
            Metric::TopHits => "top_hits",
            Metric::Terms => "terms",
        }
    }

    /// The metric of type `name`, if it is served.
    pub fn from_name(name: &str) -> Option<Metric> {
        Metric::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// One metric aggregation of a request.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricAgg {
    /// The aggregation's name, its key in the response.
    pub name: String,
    /// What it computes.
    pub metric: Metric,
    /// The field it reads.
    pub field: String,
    /// A `cardinality`'s options; the defaults for every other metric.
    pub cardinality: CardinalityOptions,
    /// A `percentiles`' or `percentile_ranks`' options; the defaults, with
    /// no keys, for every other metric.
    pub percentiles: PercentilesOptions,
    /// A `top_hits`' options; `None` for every other metric (whose `field`
    /// is the one it reads).
    pub top_hits: Option<Box<TopHitsSpec>>,
    /// A `terms`' options; `None` for every other aggregation.
    pub terms: Option<Box<TermsSpec>>,
}

/// What a `cardinality` aggregation sets besides its field.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardinalityOptions {
    /// `precision_threshold`: the count below which the sketch counts
    /// exactly; `None` for the default precision, 14.
    pub precision_threshold: Option<i64>,
    /// `execution_hint`, checked when the shard builds the aggregator.
    pub execution_hint: Option<String>,
}

impl CardinalityOptions {
    /// `CardinalityAggregatorFactory.precision()`, or the message of the
    /// `IllegalArgumentException` the shard fails with.
    pub fn precision(&self) -> Result<u32, String> {
        self.precision_threshold
            .map_or(Ok(DEFAULT_PRECISION), hll::precision_from_threshold)
    }
}

/// `HyperLogLogPlusPlus.DEFAULT_PRECISION`.
pub const DEFAULT_PRECISION: u32 = 14;

/// The most bytes aggregating `aggs` holds beyond the hits: each
/// aggregation's result with its name, the engine's request and state for
/// it, a `cardinality`'s sketch, and a `percentiles`' digest and keys (held
/// by the request and by the result). What the caller reserves before
/// searching.
pub fn state_bytes(aggs: &[MetricAgg]) -> usize {
    let per_agg = std::mem::size_of::<Partial>()
        .saturating_add(std::mem::size_of::<lucene_aggs::bucket_aggs::AggNode>())
        .saturating_add(std::mem::size_of::<lucene_aggs::bucket_aggs::AggResult>())
        .saturating_add(std::mem::size_of::<lucene_aggs::metric_aggs::MetricState>())
        .saturating_add(std::mem::size_of::<usize>());
    aggs.iter().fold(0usize, |total, agg| {
        let sketch = match agg.metric {
            Metric::Cardinality => agg.cardinality.precision().map_or(0, hll::sketch_bytes),
            Metric::Terms => agg.terms.as_ref().map_or(0, |t| {
                t.shard_size
                    .min(crate::aggs::bucket::BUCKET_LIMIT)
                    .saturating_mul(std::mem::size_of::<(i64, u64)>())
            }),
            Metric::Percentiles | Metric::PercentileRanks => {
                tdigest::digest_bytes(agg.percentiles.compression).saturating_add(
                    agg.percentiles
                        .keys
                        .len()
                        .saturating_mul(std::mem::size_of::<[f64; 2]>()),
                )
            }
            _ => 0,
        };
        total
            .saturating_add(per_agg)
            .saturating_add(sketch)
            .saturating_add(agg.name.len())
            .saturating_add(agg.field.len())
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod cardinality_tests;

#[cfg(test)]
mod top_hits_tests;

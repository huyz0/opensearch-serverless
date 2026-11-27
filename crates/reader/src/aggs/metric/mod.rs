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
pub mod collect_global;
pub mod date_text;
pub mod double;
pub mod hash_order;
pub mod hll;
pub mod keyword_cardinality;
pub mod long_value;
pub mod parse;
pub mod partial;
pub mod percentiles;
mod registered;
pub mod render;
pub mod resolve;
pub mod sub_rules;
pub mod tdigest;
pub mod top_hits;
pub mod top_hits_sub;
pub mod typed;

pub use crate::aggs::partial::{Format, Partial, Value};
pub use collect::collect;
pub use collect_global::collect_with_global;
pub use parse::parse_aggregations;
pub use percentiles::PercentilesOptions;
pub use render::{
    write_aggregations, write_aggregations_piped, write_aggregations_typed, write_aggregations_with,
};
pub use top_hits::{TopHitsResult, TopHitsSpec};

use crate::aggs::bucket::composite::CompositeSpec;
use crate::aggs::bucket::date_histogram::DateHistogramSpec;
use crate::aggs::bucket::filters::FiltersSpec;
use crate::aggs::bucket::histogram::HistogramSpec;
use crate::aggs::bucket::range::RangeSpec;
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
    /// `histogram`: the documents counted by the interval of a numeric
    /// field's values they fall in.
    Histogram,
    /// `date_histogram`: the documents counted by the calendar or fixed
    /// interval of a date field's values they fall in.
    DateHistogram,
    /// `range`: the documents counted in each of given ranges of a numeric
    /// field.
    Range,
    /// `filters`: the documents counted by each of given queries they match.
    Filters,
    /// `filter`: the documents counted that match one query.
    Filter,
    /// `composite`: the documents counted by each combination of the values
    /// of its sources, a page at a time.
    Composite,
    /// `nested`: the nested documents of a path, counted and aggregated
    /// in one bucket.
    Nested,
    /// `reverse_nested`: the parents of the nested documents collected,
    /// counted and aggregated in one bucket.
    ReverseNested,
    /// `date_range`: the documents counted in each of given ranges of a
    /// date field, its bounds read as dates.
    DateRange,
    /// `global`: every live document of the shard, whatever the query,
    /// counted and aggregated in one bucket.
    Global,
    /// `missing`: the documents counted that have no value for a field.
    Missing,
    /// `significant_terms`: the terms of a keyword field that the matches
    /// hold far more often than the whole shard does.
    SignificantTerms,
    /// `rare_terms`: the terms of a keyword field held by at most
    /// `max_doc_count` of the matches.
    RareTerms,
    /// `multi_terms`: the documents counted by each combination of the
    /// values of several keyword or long fields.
    MultiTerms,
}

impl Metric {
    /// Every metric served, by its aggregation type name.
    pub const ALL: [Metric; 25] = [
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
        Metric::Histogram,
        Metric::DateHistogram,
        Metric::Range,
        Metric::Filters,
        Metric::Filter,
        Metric::Composite,
        Metric::Nested,
        Metric::ReverseNested,
        Metric::Missing,
        Metric::DateRange,
        Metric::Global,
        Metric::SignificantTerms,
        Metric::RareTerms,
        Metric::MultiTerms,
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
            Metric::Histogram => "histogram",
            Metric::DateHistogram => "date_histogram",
            Metric::Range => "range",
            Metric::Filters => "filters",
            Metric::Filter => "filter",
            Metric::Composite => "composite",
            Metric::Nested => "nested",
            Metric::ReverseNested => "reverse_nested",
            Metric::Missing => "missing",
            Metric::DateRange => "date_range",
            Metric::Global => "global",
            Metric::SignificantTerms => "significant_terms",
            Metric::RareTerms => "rare_terms",
            Metric::MultiTerms => "multi_terms",
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
    /// A `histogram`'s options; `None` for every other aggregation.
    pub histogram: Option<Box<HistogramSpec>>,
    /// A `date_histogram`'s options; `None` for every other aggregation.
    pub date_histogram: Option<Box<DateHistogramSpec>>,
    /// The sub-aggregations of a keyword `terms`, in request order; empty
    /// for every other aggregation.
    pub subs: Vec<MetricAgg>,
    /// A `range`'s options; `None` for every other aggregation.
    pub range: Option<Box<RangeSpec>>,
    /// A `filters`' options; `None` for every other aggregation.
    pub filters: Option<Box<FiltersSpec>>,
    /// A `nested`'s or `reverse_nested`'s options; `None` for every other
    /// aggregation.
    pub nested: Option<Box<crate::aggs::bucket::nested_agg::NestedSpec>>,
    /// A `composite`'s options; `None` for every other aggregation.
    pub composite: Option<Box<CompositeSpec>>,
    /// A `significant_terms`' options; `None` for every other aggregation.
    pub significant: Option<Box<crate::aggs::bucket::significant::SignificantSpec>>,
    /// A `rare_terms`' options; `None` for every other aggregation.
    pub rare: Option<Box<crate::aggs::bucket::rare::RareSpec>>,
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

/// What one bucket of a `histogram` holds while collecting: its key, its
/// count and the engine's hash entry and ordinal.
const HISTOGRAM_BUCKET_BYTES: usize = 64;

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
            // The engine counts every key against the bucket limit, so a
            // shard holds at most that many.
            Metric::Histogram | Metric::DateHistogram => {
                crate::aggs::bucket::BUCKET_LIMIT.saturating_mul(HISTOGRAM_BUCKET_BYTES)
            }
            // A bucket, the engine's bounds and count, and each key, kept
            // by the request and by the result.
            Metric::Range | Metric::DateRange => agg.range.as_ref().map_or(0, |r| {
                let per_range = std::mem::size_of::<crate::aggs::bucket::range::RangeBucket>()
                    .saturating_add(std::mem::size_of::<[f64; 2]>())
                    .saturating_add(std::mem::size_of::<u64>());
                let keys = r.ranges.iter().fold(0usize, |n, d| {
                    n.saturating_add(d.key.as_ref().map_or(0, String::len).saturating_mul(2))
                });
                r.ranges
                    .len()
                    .min(crate::aggs::bucket::BUCKET_LIMIT)
                    .saturating_mul(per_range)
                    .saturating_add(keys)
            }),
            Metric::Composite | Metric::MultiTerms => agg.composite.as_deref().map_or(0, |spec| {
                crate::aggs::bucket::composite_collect::state_bytes(spec, agg.subs.len())
            }),
            Metric::Filters | Metric::Filter | Metric::Missing | Metric::Global => agg
                .filters
                .as_deref()
                .map_or(0, crate::aggs::bucket::filters_collect::state_bytes),
            Metric::Nested | Metric::ReverseNested => {
                crate::aggs::bucket::nested_agg::state_bytes()
            }
            Metric::Percentiles | Metric::PercentileRanks => {
                tdigest::digest_bytes(agg.percentiles.compression).saturating_add(
                    agg.percentiles
                        .keys
                        .len()
                        .saturating_mul(std::mem::size_of::<[f64; 2]>()),
                )
            }
            Metric::SignificantTerms => agg.significant.as_deref().map_or(0, |s| {
                s.size
                    .min(crate::aggs::bucket::BUCKET_LIMIT)
                    .saturating_mul(std::mem::size_of::<
                        crate::aggs::bucket::significant::SigBucket,
                    >())
            }),
            Metric::RareTerms => crate::aggs::bucket::BUCKET_LIMIT
                .saturating_mul(std::mem::size_of::<(String, u64)>()),
            _ => 0,
        };
        total
            .saturating_add(per_agg)
            .saturating_add(sketch)
            .saturating_add(agg.name.len())
            .saturating_add(agg.field.len())
            .saturating_add(state_bytes(&agg.subs))
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod cardinality_tests;

#[cfg(test)]
mod collect_kind_tests;

#[cfg(test)]
mod top_hits_tests;

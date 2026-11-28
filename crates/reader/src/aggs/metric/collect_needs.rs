//! What each metric reads of its field's engine state.

use lucene_aggs::metric_aggs::{NEED_COUNT, NEED_MAX, NEED_MIN, NEED_MIN_MAX, NEED_SQ, NEED_SUM};

use crate::aggs::metric::Metric;

/// What `avg` reads: the sum and the count.
const AVG_NEEDS: u8 = NEED_SUM + NEED_COUNT;

/// What `stats` reads: the count, the sum, the smallest and the largest.
const STATS_NEEDS: u8 = NEED_COUNT + NEED_SUM + NEED_MIN_MAX;

/// What `extended_stats` reads: `stats`' and the sum of squares.
const EXTENDED_NEEDS: u8 = STATS_NEEDS + NEED_SQ;

/// What a metric reads of its field's state.
pub(super) fn needs(metric: Metric) -> u8 {
    match metric {
        Metric::Min => NEED_MIN,
        Metric::Max => NEED_MAX,
        Metric::Sum => NEED_SUM,
        Metric::Avg => AVG_NEEDS,
        Metric::Stats => STATS_NEEDS,
        Metric::ExtendedStats => EXTENDED_NEEDS,
        Metric::ValueCount
        | Metric::Cardinality
        | Metric::Percentiles
        | Metric::PercentileRanks
        | Metric::TopHits
        | Metric::Terms
        | Metric::SignificantTerms
        | Metric::RareTerms
        | Metric::Histogram
        | Metric::DateHistogram
        | Metric::AutoDateHistogram => NEED_COUNT,
        Metric::Range
        | Metric::DateRange
        | Metric::Filters
        | Metric::AdjacencyMatrix
        | Metric::Filter
        | Metric::Missing
        | Metric::Global
        | Metric::Composite
        | Metric::MultiTerms
        | Metric::Nested
        | Metric::ReverseNested
        | Metric::ScriptedMetric
        | Metric::GeoCentroid
        | Metric::GeoBounds => NEED_COUNT,
    }
}

//! Which aggregations may sit under which: the bucket aggregations this node
//! runs sub-aggregations under, and what each may hold.

use crate::aggs::metric::Metric;

/// Whether `metric` is a bucket aggregation this node runs sub-aggregations
/// under.
pub(super) fn holds_subs(metric: Metric) -> bool {
    matches!(
        metric,
        Metric::Terms
            | Metric::Range
            | Metric::DateRange
            | Metric::Filters
            | Metric::Filter
            | Metric::Missing
            | Metric::Histogram
            | Metric::DateHistogram
            | Metric::Composite
            | Metric::Nested
            | Metric::ReverseNested
    )
}

/// Whether a bucket aggregation `sub` may sit under the bucket aggregation
/// `parent`: a `range`, `histogram` or `date_histogram` under a `range`,
/// `filters`, `filter`, `histogram`, `date_histogram` or `terms` (which the
/// shard refuses for a keyword field) so far.
pub(super) fn nests_under(sub: Metric, parent: Metric) -> bool {
    let in_nested = matches!(parent, Metric::Nested | Metric::ReverseNested);
    // A `nested` or `reverse_nested` runs beneath another one or a `terms`
    // (a `reverse_nested` outside any `nested` is refused when planned).
    if matches!(sub, Metric::Nested | Metric::ReverseNested) {
        return in_nested || parent == Metric::Terms;
    }
    matches!(
        sub,
        Metric::Range
            | Metric::DateRange
            | Metric::Filters
            | Metric::Filter
            | Metric::Missing
            | Metric::Histogram
            | Metric::DateHistogram
    ) && (in_nested
        || matches!(
            parent,
            Metric::Range
                | Metric::DateRange
                | Metric::Filters
                | Metric::Filter
                | Metric::Missing
                | Metric::Histogram
                | Metric::DateHistogram
                | Metric::Terms
        ))
}

/// Whether `metric` is a single-value or stats metric, one of the
/// sub-aggregations of a `terms` this node runs (a `terms` is the other).
pub(super) fn is_scalar(metric: Metric) -> bool {
    matches!(
        metric,
        Metric::Min | Metric::Max | Metric::Sum | Metric::Avg | Metric::Stats | Metric::ValueCount
    )
}

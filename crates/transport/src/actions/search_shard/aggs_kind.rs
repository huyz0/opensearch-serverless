//! The kinds of aggregation a forwarded search may ask for.

/// The aggregations a forwarded search may ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggKind {
    /// `min`.
    Min,
    /// `max`.
    Max,
    /// `sum`.
    Sum,
    /// `avg`.
    Avg,
    /// `value_count`.
    ValueCount,
    /// `stats`.
    Stats,
    /// `cardinality`.
    Cardinality,
    /// `percentiles`.
    Percentiles,
    /// `percentile_ranks`.
    PercentileRanks,
    /// `terms`.
    Terms,
    /// `date_histogram`.
    DateHistogram,
    /// `range`.
    Range,
    /// `date_range`.
    DateRange,
    /// `rare_terms`.
    RareTerms,
    /// `multi_terms`.
    MultiTerms,
}

impl AggKind {
    /// The aggregation's type name, which is its writeable name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Min => "min",
            Self::Max => "max",
            Self::Sum => "sum",
            Self::Avg => "avg",
            Self::ValueCount => "value_count",
            Self::Stats => "stats",
            Self::Cardinality => "cardinality",
            Self::Percentiles => "percentiles",
            Self::PercentileRanks => "percentile_ranks",
            Self::Terms => "terms",
            Self::DateHistogram => "date_histogram",
            Self::Range => "range",
            Self::DateRange => "date_range",
            Self::RareTerms => "rare_terms",
            Self::MultiTerms => "multi_terms",
        }
    }

    pub(super) fn from_name(name: &str) -> Option<Self> {
        [
            Self::Min,
            Self::Max,
            Self::Sum,
            Self::Avg,
            Self::ValueCount,
            Self::Stats,
            Self::Cardinality,
            Self::Percentiles,
            Self::PercentileRanks,
            Self::Terms,
            Self::DateHistogram,
            Self::Range,
            Self::DateRange,
            Self::RareTerms,
            Self::MultiTerms,
        ]
        .into_iter()
        .find(|k| k.name() == name)
    }
}

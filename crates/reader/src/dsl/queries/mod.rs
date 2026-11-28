//! The queries beyond the first subset: term-level queries (`terms`,
//! `prefix`, `wildcard`, `regexp`, `fuzzy`, `exists`, `ids`) and compound
//! ones (`constant_score`, `dis_max`, `match_none`), and the full-text
//! `match_phrase` and `multi_match`.
//!
//! Each parser is ported from its builder's `fromXContent` at OpenSearch
//! 3.10.0 (server/src/main/java/org/opensearch/index/query/), with the same
//! messages; [`build`] holds their `doToQuery`. A parameter OpenSearch takes
//! but this reader does not serve yet is refused rather than ignored.

pub mod bool_prefix;
pub mod build;
pub mod case_insensitive;
pub mod compound;
pub mod cross_fields;
pub mod full_text;
pub mod full_text_build;
pub mod function_score;
pub mod function_score_build;
mod function_score_modes;
pub(crate) mod function_score_painless;
pub mod function_score_random;
pub mod function_score_script;
pub mod geo_distance;
pub mod inner_hits;
pub mod knn;
mod leaf_common;
pub mod nested;
pub mod nested_build;
pub mod phrase_prefix;
pub mod query_string;
pub mod query_string_build;
pub mod script_query;
pub mod simple_string;
pub mod simple_string_build;
mod simple_string_leaf;
pub mod single_field;
pub mod sltr;
pub mod term_level;
pub mod uid;

use crate::dsl::error::DslError;
use crate::dsl::query::Query;
use crate::dsl::tokens::Parser;
use crate::dsl::values::Value;

/// The names [`parse`] reads.
pub const NAMES: &[&str] = &[
    "terms",
    "prefix",
    "wildcard",
    "regexp",
    "fuzzy",
    "exists",
    "ids",
    "constant_score",
    "dis_max",
    "boosting",
    "match_none",
    "match_phrase",
    "match_bool_prefix",
    "match_phrase_prefix",
    "multi_match",
    "simple_query_string",
    "query_string",
    "function_score",
    "knn",
    "geo_distance",
    "nested",
    "sltr",
];

/// `Fuzziness`: an edit distance, or `AUTO` with its two length bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fuzziness {
    /// A fixed distance: 0, 1 or 2.
    Edits(u8),
    /// `AUTO:low,high`: 0 edits below `low` characters, 1 below `high`, 2
    /// from there.
    Auto {
        /// Terms shorter than this take no edit.
        low: i32,
        /// Terms shorter than this take one edit.
        high: i32,
    },
}

impl Fuzziness {
    /// `Fuzziness.AUTO`, the default.
    pub const AUTO: Fuzziness = Fuzziness::Auto { low: 3, high: 6 };

    /// `Fuzziness.asDistance(text)`: the edits for a term, its length
    /// counted in code points as `termLen` counts.
    pub fn as_distance(self, text: &str) -> u8 {
        match self {
            Fuzziness::Edits(n) => n,
            Fuzziness::Auto { low, high } => {
                let len = i64::try_from(text.chars().count()).unwrap_or(i64::MAX);
                if len < i64::from(low) {
                    0
                } else if len < i64::from(high) {
                    1
                } else {
                    2
                }
            }
        }
    }
}

/// A parsed query of this module.
#[derive(Clone, Debug, PartialEq)]
pub enum More {
    /// `TermsQueryBuilder` with values (no terms lookup).
    Terms {
        /// The field.
        field: String,
        /// The values; empty matches nothing.
        values: Vec<Value>,
        /// The boost.
        boost: f32,
    },
    /// `PrefixQueryBuilder`.
    Prefix {
        /// The field.
        field: String,
        /// The prefix.
        value: String,
        /// The boost.
        boost: f32,
    },
    /// `WildcardQueryBuilder`.
    Wildcard {
        /// The field.
        field: String,
        /// The pattern: `*` any run, `?` one character, `\` an escape.
        value: String,
        /// The boost.
        boost: f32,
    },
    /// `RegexpQueryBuilder` with the default flags.
    Regexp {
        /// The field.
        field: String,
        /// The regular expression, matched against whole terms.
        value: String,
        /// The boost.
        boost: f32,
    },
    /// `FuzzyQueryBuilder`.
    Fuzzy {
        /// The field.
        field: String,
        /// The term.
        value: Value,
        /// How many edits.
        fuzziness: Fuzziness,
        /// How many leading characters must match exactly.
        prefix_length: i32,
        /// The most terms the query expands to.
        max_expansions: i32,
        /// Whether a swap of two adjacent characters is one edit.
        transpositions: bool,
        /// The boost.
        boost: f32,
    },
    /// `ExistsQueryBuilder`.
    Exists {
        /// The field name or pattern.
        field: String,
        /// The boost.
        boost: f32,
    },
    /// `IdsQueryBuilder`.
    Ids {
        /// The ids.
        values: Vec<String>,
        /// The boost.
        boost: f32,
    },
    /// A `term`, `prefix` or `wildcard` query with `case_insensitive: true`.
    CaseInsensitive {
        /// Which query.
        kind: case_insensitive::Kind,
        /// The field.
        field: String,
        /// The term, prefix or pattern.
        value: String,
        /// The boost.
        boost: f32,
    },
    /// `ConstantScoreQueryBuilder`.
    ConstantScore {
        /// The filter.
        filter: Query,
        /// The boost, which is the score.
        boost: f32,
    },
    /// `DisMaxQueryBuilder`.
    DisMax {
        /// The disjuncts.
        queries: Vec<Query>,
        /// How much of the other disjuncts' scores is added to the best.
        tie_breaker: f32,
        /// The boost.
        boost: f32,
    },
    /// `BoostingQueryBuilder`.
    Boosting {
        /// The query that matches.
        positive: Query,
        /// The query whose matches have their score scaled.
        negative: Query,
        /// The factor a match of `negative` scales the score by.
        negative_boost: f32,
        /// The boost.
        boost: f32,
    },
    /// `MatchNoneQueryBuilder`.
    MatchNone {
        /// The boost.
        boost: f32,
    },
    /// `MatchPhraseQueryBuilder`.
    MatchPhrase {
        /// The field.
        field: String,
        /// The text.
        query: Value,
        /// How many positions the terms may move.
        slop: u32,
        /// `zero_terms_query: all`.
        zero_terms_all: bool,
        /// The boost.
        boost: f32,
    },
    /// `MatchQueryBuilder` with a `fuzziness`, and its boost.
    FuzzyMatch(Box<full_text::FuzzyMatch>, f32),
    /// `MatchBoolPrefixQueryBuilder`, and its boost.
    MatchBoolPrefix(Box<bool_prefix::BoolPrefix>, f32),
    /// `MatchPhrasePrefixQueryBuilder`, and its boost.
    MatchPhrasePrefix(Box<phrase_prefix::PhrasePrefix>, f32),
    /// `MultiMatchQueryBuilder` of the types served, and its boost.
    MultiMatch(Box<full_text::MultiMatch>, f32),
    /// `SimpleQueryStringBuilder`, and its boost.
    SimpleQueryString(Box<simple_string::SimpleQueryString>, f32),
    /// `QueryStringQueryBuilder`, and its boost.
    QueryString(Box<query_string::QueryString>, f32),
    /// `FunctionScoreQueryBuilder`, and its boost.
    FunctionScore(Box<function_score::FunctionScore>, f32),
    /// `NestedQueryBuilder`, with its boost.
    Nested(Box<nested::NestedQuery>, f32),
    /// `ScriptQueryBuilder` over Painless, and its boost.
    Script(Box<function_score_script::ScriptScore>, f32),
    /// `GeoDistanceQueryBuilder`.
    GeoDistance(Box<geo_distance::GeoDistance>),
    /// The k-NN plugin's `KNNQueryBuilder`.
    Knn(Box<knn::Knn>),
    /// The learning to rank plugin's `sltr`, served as a rescore query.
    Sltr(Box<sltr::Sltr>),
}

impl More {
    /// The query's boost.
    pub fn boost(&self) -> f32 {
        match self {
            More::Terms { boost, .. }
            | More::Prefix { boost, .. }
            | More::Wildcard { boost, .. }
            | More::CaseInsensitive { boost, .. }
            | More::Regexp { boost, .. }
            | More::Fuzzy { boost, .. }
            | More::Exists { boost, .. }
            | More::Ids { boost, .. }
            | More::ConstantScore { boost, .. }
            | More::DisMax { boost, .. }
            | More::Boosting { boost, .. }
            | More::MatchPhrase { boost, .. }
            | More::MultiMatch(_, boost)
            | More::FuzzyMatch(_, boost)
            | More::MatchBoolPrefix(_, boost)
            | More::MatchPhrasePrefix(_, boost)
            | More::SimpleQueryString(_, boost)
            | More::QueryString(_, boost)
            | More::FunctionScore(_, boost)
            | More::Nested(_, boost)
            | More::Script(_, boost)
            | More::MatchNone { boost } => *boost,
            More::Knn(q) => q.boost,
            More::GeoDistance(q) => q.boost,
            More::Sltr(_) => 1.0,
        }
    }
}

/// Parses query `name`, one of [`NAMES`]: the parser is on the query
/// object's start, and ends on its end.
pub fn parse(name: &str, p: &mut Parser<'_>) -> Result<More, DslError> {
    match name {
        "terms" => term_level::parse_terms(p),
        "prefix" => term_level::parse_prefix(p),
        "wildcard" => term_level::parse_wildcard(p),
        "regexp" => term_level::parse_regexp(p),
        "fuzzy" => term_level::parse_fuzzy(p),
        "exists" => term_level::parse_exists(p),
        "ids" => term_level::parse_ids(p),
        "constant_score" => compound::parse_constant_score(p),
        "dis_max" => compound::parse_dis_max(p),
        "boosting" => compound::parse_boosting(p),
        "match_phrase" => full_text::parse_match_phrase(p),
        "match_bool_prefix" => bool_prefix::parse(p),
        "match_phrase_prefix" => phrase_prefix::parse(p),
        "multi_match" => full_text::parse_multi_match(p),
        "simple_query_string" => simple_string::parse_simple_query_string(p),
        "query_string" => query_string::parse_query_string(p),
        "function_score" => function_score::parse_function_score(p),
        "knn" => knn::parse_knn(p),
        // ALLOC: one boxed query per request, bounded by the body's recording.
        "geo_distance" => geo_distance::parse(p).map(|q| More::GeoDistance(Box::new(q))),
        "sltr" => sltr::parse_sltr(p),
        "nested" => nested::parse_nested(p),
        _ => compound::parse_match_none(p),
    }
}

#[cfg(test)]
mod named_tests;
#[cfg(test)]
mod parse_tests;

#[cfg(test)]
mod compound_tests;

#[cfg(test)]
mod geo_distance_tests;
#[cfg(test)]
mod knn_tests;

#[cfg(test)]
mod sltr_tests;

#[cfg(test)]
#[path = "inner_hits_tests.rs"]
mod inner_hits_tests;
#[cfg(test)]
mod nested_tests;

#[cfg(test)]
mod build_tests;

#[cfg(test)]
mod full_text_tests;

#[cfg(test)]
mod function_score_build_tests;
#[cfg(test)]
mod fuzzy_match_tests;

#[cfg(test)]
mod function_score_tests;

#[cfg(test)]
mod script_switch_tests;

#[cfg(test)]
mod query_string_build_tests;

#[cfg(test)]
mod query_string_tests;

#[cfg(test)]
mod simple_string_build_tests;

#[cfg(test)]
mod simple_string_tests;

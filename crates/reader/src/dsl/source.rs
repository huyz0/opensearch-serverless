//! A search request's source: `SearchSourceBuilder.parseXContent` for
//! `query`, `size`, `from`, `sort`, `track_total_hits`, `_source`,
//! `search_after` and `docvalue_fields`, and the `size` and `from` URL
//! parameters of `RestSearchAction.parseSearchSource`, at OpenSearch 3.10.0.
//!
//! Every other key OpenSearch reads is refused as not served yet; a key it
//! does not know fails as it fails there ("Unknown key for a ... in [..].").

use oss_rest::json::{Limits, Tokenizer};

use crate::aggs::metric::{MetricAgg, parse_aggregations};
use crate::dsl::error::DslError;
use crate::dsl::pit::{PitRef, parse_pit};
use crate::dsl::query::{Query, parse_inner};
use crate::dsl::search_after::{AfterValue, parse_search_after};
use crate::dsl::sort::{SortKey, parse_sort};
use crate::dsl::tokens::{Kind, Parser, Recorder};
use crate::dsl::values::{bool_value, int_value};
use crate::fetch::docvalue_fields::{DocValueField, parse_docvalue_fields};
use crate::fetch::highlight_parse::{HighlightSpec, parse_highlight};
use crate::fetch::source_filter::{SourceFilter, parse_source};

/// `SearchService.DEFAULT_SIZE`.
pub const DEFAULT_SIZE: usize = 10;

/// The top-level keys OpenSearch reads whose value is a scalar.
const VALUE_KEYS: &[&str] = &[
    "timeout",
    "terminate_after",
    "min_score",
    "track_scores",
    "include_named_queries_score",
    "track_total_hits",
    "_source",
    "stored_fields",
    "profile",
    "search_pipeline",
    "verbose_pipeline",
];

/// The top-level keys OpenSearch reads whose value is an object.
const OBJECT_KEYS: &[&str] = &[
    "post_filter",
    "_source",
    "script_fields",
    "indices_boost",
    "aggregations",
    "aggs",
    "suggest",
    "rescore",
    "ext",
    "slice",
    "collapse",
    "search_pipeline",
    "derived",
];

/// The top-level keys OpenSearch reads whose value is an array.
const ARRAY_KEYS: &[&str] = &[
    "stored_fields",
    "docvalue_fields",
    "fields",
    "indices_boost",
    "rescore",
    "stats",
    "_source",
    "search_after",
];

/// What a search asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchSource {
    /// The query; match-all when absent.
    pub query: Option<Query>,
    /// How many hits to skip.
    pub from: usize,
    /// How many hits to return.
    pub size: usize,
    /// The sort; by score when empty.
    pub sort: Vec<SortKey>,
    /// `track_total_hits`: matches are counted exactly up to this many;
    /// `None` reports no total.
    pub track_total_hits: Option<u64>,
    /// `_source`: whether and which of each hit's source to return.
    pub source: SourceFilter,
    /// The aggregations, in request order; none when empty.
    pub aggs: Vec<MetricAgg>,
    /// `search_after`: the sort values the page starts after; none when
    /// empty.
    pub search_after: Vec<AfterValue>,
    /// `docvalue_fields`: the fields whose doc values each hit carries.
    pub docvalue_fields: Vec<DocValueField>,
    /// `explain`: whether each hit carries the explanation of its score.
    pub explain: bool,
    /// `version`: whether each hit carries its `_version`.
    pub version: bool,
    /// `seq_no_primary_term`: whether each hit carries its `_seq_no` and
    /// `_primary_term`.
    pub seq_no_primary_term: bool,
    /// `highlight`: the fields whose matches each hit marks.
    pub highlight: Option<HighlightSpec>,
    /// `pit`: the point in time the search runs on.
    pub pit: Option<PitRef>,
}

impl Default for SearchSource {
    fn default() -> Self {
        Self {
            query: None,
            from: 0,
            size: DEFAULT_SIZE,
            // ALLOC: empty; a sort pushes at most MAX_SORT_KEYS.
            sort: Vec::new(),
            track_total_hits: Some(DEFAULT_TRACK_TOTAL_HITS),
            source: SourceFilter::default(),
            // ALLOC: empty; parsing pushes one an aggregation.
            aggs: Vec::new(),
            // ALLOC: empty; parsing pushes one a value.
            search_after: Vec::new(),
            // ALLOC: empty; parsing pushes one an entry.
            docvalue_fields: Vec::new(),
            explain: false,
            version: false,
            seq_no_primary_term: false,
            highlight: None,
            pit: None,
        }
    }
}

/// `SearchContext.DEFAULT_TRACK_TOTAL_HITS_UP_TO`.
pub const DEFAULT_TRACK_TOTAL_HITS: u64 = 10_000;

/// `track_total_hits`: a boolean (or a string of one), else an int, `-1`
/// turning the total off as `false` does.
fn track_total_hits(p: &Parser<'_>) -> Result<Option<u64>, DslError> {
    if p.current_token() == Some(Kind::Bool) || matches!(p.text(), "true" | "false") {
        return Ok(bool_value(p)?.then_some(u64::from(i32::MAX.cast_unsigned())));
    }
    match int_value(p)? {
        -1 => Ok(None),
        // OpenSearch accepts it here and fails the fetch phase.
        n if n < 0 => Err(DslError::unsupported("a [track_total_hits] below -1")),
        n => Ok(Some(u64::from(n.cast_unsigned()))),
    }
}

/// `from(int)`: refuses a negative value.
fn non_negative(name: &str, n: i32) -> Result<usize, DslError> {
    usize::try_from(n).map_err(|_| {
        // ALLOC: error path; one message.
        DslError::illegal(format!(
            "[{name}] parameter cannot be negative, found [{n}]"
        ))
    })
}

impl SearchSource {
    /// `SearchSourceBuilder.parseXContent(parser, true)` over a recorded
    /// body; an empty body is the default source.
    pub fn parse(rec: &Recorder) -> Result<Self, DslError> {
        let mut source = SearchSource::default();
        if rec.is_empty() {
            return Ok(source);
        }
        let mut p = rec.parser();
        let token = p.next_token();
        if token != Some(Kind::StartObject) {
            return Err(DslError::parsing(
                p.location(),
                // ALLOC: error path; one message.
                format!(
                    "Expected [START_OBJECT] but found [{}]",
                    token.map_or("null", Kind::name)
                ),
            ));
        }
        let mut name = "";
        while let Some(token) = p.next_token() {
            match token {
                Kind::EndObject => break,
                Kind::FieldName => name = p.text(),
                k if k.is_value() => source.value(&p, name, k)?,
                Kind::StartObject => source.object(&mut p, name)?,
                Kind::StartArray => source.array(&mut p, name)?,
                k => return Err(unknown_key(&p, k, name)),
            }
        }
        // The tokenizer refuses anything after the root value, so nothing
        // follows the main object here.
        Ok(source)
    }

    fn value(&mut self, p: &Parser<'_>, name: &str, kind: Kind) -> Result<(), DslError> {
        match name {
            "from" => self.from = non_negative("from", int_value(p)?)?,
            "size" => self.size = non_negative("size", int_value(p)?)?,
            "sort" => self.sort = parse_sort(&mut p.clone())?,
            // `SearchSourceBuilder`'s value branch takes no null.
            "_source" | "track_total_hits" | "explain" | "version" | "seq_no_primary_term"
                if kind == Kind::Null =>
            {
                return Err(unknown_key(p, kind, name));
            }
            "_source" => self.source = parse_source(&mut p.clone(), name)?,
            "track_total_hits" => self.track_total_hits = track_total_hits(p)?,
            "explain" => self.explain = bool_value(p)?,
            "version" => self.version = bool_value(p)?,
            "seq_no_primary_term" => self.seq_no_primary_term = bool_value(p)?,
            known if VALUE_KEYS.contains(&known) => return Err(unsupported(known)),
            _ => return Err(unknown_key(p, kind, name)),
        }
        Ok(())
    }

    fn object(&mut self, p: &mut Parser<'_>, name: &str) -> Result<(), DslError> {
        match name {
            "query" => self.query = Some(parse_inner(p)?),
            "sort" => self.sort = parse_sort(p)?,
            "aggregations" | "aggs" => self.aggs = parse_aggregations(p)?,
            "_source" => self.source = parse_source(p, name)?,
            "highlight" => self.highlight = Some(parse_highlight(p)?),
            "pit" => self.pit = Some(parse_pit(p)?),
            known if OBJECT_KEYS.contains(&known) => return Err(unsupported(known)),
            _ => return Err(unknown_key(p, Kind::StartObject, name)),
        }
        Ok(())
    }

    fn array(&mut self, p: &mut Parser<'_>, name: &str) -> Result<(), DslError> {
        match name {
            "sort" => self.sort = parse_sort(p)?,
            "_source" => self.source = parse_source(p, name)?,
            "search_after" => self.search_after = parse_search_after(p)?,
            "docvalue_fields" => self.docvalue_fields = parse_docvalue_fields(p)?,
            known if ARRAY_KEYS.contains(&known) => return Err(unsupported(known)),
            _ => return Err(unknown_key(p, Kind::StartArray, name)),
        }
        Ok(())
    }

    /// `RestSearchAction.parseSearchSource`'s `from` and `size` parameters,
    /// which override the body's.
    pub fn apply_params(&mut self, from: Option<&str>, size: Option<&str>) -> Result<(), DslError> {
        for (name, value) in [("from", from), ("size", size)] {
            let Some(text) = value else { continue };
            let n = text.parse::<i32>().map_err(|_| {
                DslError::illegal(
                    // ALLOC: error path; one message.
                    format!("Failed to parse int parameter [{name}] with value [{text}]"),
                )
            })?;
            let n = non_negative(name, n)?;
            if name == "from" {
                self.from = n;
            } else {
                self.size = n;
            }
        }
        Ok(())
    }
}

fn unsupported(key: &str) -> DslError {
    // ALLOC: error path; one message.
    DslError::unsupported(&format!("the search request's [{key}]"))
}

pub(crate) fn unknown_key(p: &Parser<'_>, kind: Kind, name: &str) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!("Unknown key for a {} in [{name}].", kind.name()),
    )
}

/// Tokenizer limits for a search body: OpenSearch's, with pieces small
/// enough that a long string never needs a large buffer.
pub fn tokenizer() -> Tokenizer {
    Tokenizer::new(Limits {
        string_chunk: 4096,
        ..Limits::default()
    })
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;

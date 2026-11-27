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
use crate::dsl::values::{bool_value, float_value, int_value, is_time_value};
use crate::fetch::docvalue_fields::{DocValueField, parse_docvalue_fields};
use crate::fetch::highlight_parse::{HighlightSpec, parse_highlight};
use crate::fetch::source_filter::{SourceFilter, parse_source};

/// `SearchService.DEFAULT_SIZE`.
pub const DEFAULT_SIZE: usize = 10;

/// The top-level keys OpenSearch reads whose value is a scalar.
const VALUE_KEYS: &[&str] = &[
    "timeout",
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
    /// `fields`: the fields each hit carries the `_source` values of.
    pub fields: Vec<DocValueField>,
    /// `stored_fields`: the names to fetch; none when the key is absent (`_none_` is an empty
    /// list that also sets `source.no_id`).
    pub stored_fields: Option<Vec<String>>,
    /// Whether the body has a `_source` key.
    pub source_given: bool,
    /// `script_fields`: a script's number a hit carries, by name.
    pub script_fields: Vec<(
        String,
        crate::dsl::queries::function_score_script::ScriptScore,
    )>,
    /// `explain`: whether each hit carries the explanation of its score.
    pub explain: bool,
    /// `min_score`: only matches scoring at least this are hits and counted.
    pub min_score: Option<f32>,
    /// `post_filter`: narrows the hits and their total after the
    /// aggregations have seen every match of the query.
    pub post_filter: Option<Query>,
    /// `terminate_after`: collect only the first this many matches of the
    /// shard; 0 is off.
    pub terminate_after: u64,
    /// `track_scores`: whether a field sort still scores each hit and tracks
    /// the best score of every match.
    pub track_scores: bool,
    /// `version`: whether each hit carries its `_version`.
    pub version: bool,
    /// `seq_no_primary_term`: whether each hit carries its `_seq_no` and
    /// `_primary_term`.
    pub seq_no_primary_term: bool,
    /// `highlight`: the fields whose matches each hit marks.
    pub highlight: Option<HighlightSpec>,
    /// `rescore`: the second passes over the best hits, in order.
    pub rescore: Vec<crate::dsl::rescore::Rescorer>,
    /// `collapse`: the field whose values keep one hit each.
    pub collapse: Option<crate::dsl::collapse::Collapse>,
    /// `pit`: the point in time the search runs on.
    pub pit: Option<PitRef>,
    /// Set by a search pipeline's `truncate_hits`: the hits answered are
    /// cut to this many. The search itself still asks for `size`.
    pub response_size_cap: Option<usize>,
    /// `search_pipeline`: the stored pipeline the body names.
    pub search_pipeline: Option<String>,
    /// The instant `now` in date math means: set by a caller that has one
    /// (a forwarded search carries its coordinator's), none otherwise, and a
    /// `now` in a range is then refused.
    pub now_millis: Option<i64>,
    /// `rest_total_hits_as_int`: the total is written as a number, not an object with its relation.
    pub total_as_int: bool,
    /// `typed_keys`: each aggregation's key in the response carries its type, `min#name`.
    pub typed_keys: bool,
}

impl SearchSource {
    /// This source, or the refusal a body gets where no search pipeline runs
    /// (a by-query search, a rendered template).
    ///
    /// # Errors
    ///
    /// [`DslError`] when the body names a `search_pipeline`.
    pub fn refuse_pipeline(self) -> Result<Self, DslError> {
        match self.search_pipeline {
            Some(_) => Err(unsupported("search_pipeline")),
            None => Ok(self),
        }
    }
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
            fields: Vec::new(),
            stored_fields: None,
            source_given: false,
            // ALLOC: empty, no allocation.
            script_fields: Vec::new(),
            explain: false,
            min_score: None,
            post_filter: None,
            terminate_after: 0,
            track_scores: false,
            version: false,
            seq_no_primary_term: false,
            highlight: None,
            // ALLOC: empty; parsing pushes one a rescorer.
            rescore: Vec::new(),
            collapse: None,
            pit: None,
            response_size_cap: None,
            search_pipeline: None,
            now_millis: None,
            total_as_int: false,
            typed_keys: false,
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

/// The `stored_fields` array's names, the parser on its start.
fn stored_names(p: &mut Parser<'_>) -> Result<Vec<String>, DslError> {
    // ALLOC: one name an element, bounded by the recorded body.
    let mut names = Vec::new();
    loop {
        match p.next_token() {
            Some(Kind::EndArray) | None => return Ok(names),
            // ALLOC: as above.
            Some(Kind::String) => names.push(p.text().to_owned()),
            Some(_) => return Err(unsupported("stored_fields")),
        }
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
    /// `StoredFieldsContext.fromList` and what `FetchPhase.createStoredFieldsVisitor` makes of
    /// it: `_none_` drops the hit's `_id` and `_source`, and names drop the `_source` unless it
    /// is named or the body asks for it.
    fn apply_stored_fields(&mut self) -> Result<(), DslError> {
        let Some(names) = self.stored_fields.as_mut() else {
            return Ok(());
        };
        if names.iter().any(|n| n == "_none_") {
            if names.len() > 1 {
                return Err(DslError::unsupported(
                    "[stored_fields] of [_none_] with other names",
                ));
            }
            if (self.source_given && !self.source.skip) || !self.fields.is_empty() {
                return Err(DslError::unsupported(
                    "[stored_fields] of [_none_] with a [_source] or [fields]",
                ));
            }
            names.clear();
            self.source.skip = true;
            self.source.no_id = true;
        } else if names.iter().any(|n| n == "_source") {
            self.source.skip = false;
        } else if !self.source_given {
            self.source.skip = true;
        }
        Ok(())
    }

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
        source.apply_stored_fields()?;
        Ok(source)
    }

    fn value(&mut self, p: &Parser<'_>, name: &str, kind: Kind) -> Result<(), DslError> {
        match name {
            "from" => self.from = non_negative("from", int_value(p)?)?,
            "size" => self.size = non_negative("size", int_value(p)?)?,
            "sort" => self.sort = parse_sort(&mut p.clone())?,
            // `SearchSourceBuilder`'s value branch takes no null.
            "_source"
            | "track_total_hits"
            | "explain"
            | "version"
            | "seq_no_primary_term"
            | "min_score"
            | "terminate_after"
            | "track_scores"
            | "timeout"
                if kind == Kind::Null =>
            {
                return Err(unknown_key(p, kind, name));
            }
            "_source" => {
                self.source = parse_source(&mut p.clone(), name)?;
                self.source_given = true;
            }
            "track_total_hits" => self.track_total_hits = track_total_hits(p)?,
            "explain" => self.explain = bool_value(p)?,
            "min_score" => self.min_score = Some(float_value(p)?),
            "terminate_after" => {
                let n = int_value(p)?;
                self.terminate_after = u64::try_from(n).map_err(|_| {
                    // ALLOC: error path; one message.
                    DslError::illegal("terminateAfter must be > 0".to_owned())
                })?;
            }
            "track_scores" => self.track_scores = bool_value(p)?,
            "version" => self.version = bool_value(p)?,
            "seq_no_primary_term" => self.seq_no_primary_term = bool_value(p)?,
            "search_pipeline" if kind == Kind::String && !p.text().is_empty() => {
                // ALLOC: one id, bounded by the recorded body.
                self.search_pipeline = Some(p.text().to_owned());
            }
            // A time limit changes nothing here: a search is not cut short.
            "timeout" if matches!(kind, Kind::String | Kind::Number) && is_time_value(p.text()) => {
            }
            "stored_fields" if kind == Kind::String => {
                // ALLOC: one name, bounded by the recorded body.
                self.stored_fields = Some(vec![p.text().to_owned()]);
            }
            // A profile is not built, so only the default of no profile is read.
            "profile" if kind == Kind::Bool && p.text() == "false" => {}
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
            "_source" => {
                self.source = parse_source(p, name)?;
                self.source_given = true;
            }
            "highlight" => self.highlight = Some(parse_highlight(p)?),
            "post_filter" => self.post_filter = Some(parse_inner(p)?),
            "pit" => self.pit = Some(parse_pit(p)?),
            "script_fields" => self.script_fields = crate::fetch::script_fields::parse(p)?,
            "rescore" => self.rescore = crate::dsl::rescore::parse_object(p)?,
            "collapse" => self.collapse = Some(crate::dsl::collapse::parse(p)?),
            known if OBJECT_KEYS.contains(&known) => return Err(unsupported(known)),
            _ => return Err(unknown_key(p, Kind::StartObject, name)),
        }
        Ok(())
    }

    fn array(&mut self, p: &mut Parser<'_>, name: &str) -> Result<(), DslError> {
        match name {
            "sort" => self.sort = parse_sort(p)?,
            "_source" => {
                self.source = parse_source(p, name)?;
                self.source_given = true;
            }
            "search_after" => self.search_after = parse_search_after(p)?,
            "stored_fields" => self.stored_fields = Some(stored_names(p)?),
            "docvalue_fields" => self.docvalue_fields = parse_docvalue_fields(p)?,
            "fields" => self.fields = parse_docvalue_fields(p)?,
            "rescore" => self.rescore = crate::dsl::rescore::parse_array(p)?,
            // Group names for the stats API, which this node does not keep.
            "stats" => loop {
                match p.next_token() {
                    Some(Kind::EndArray) | None => break,
                    Some(Kind::String) => {}
                    Some(_) => return Err(unsupported(name)),
                }
            },
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

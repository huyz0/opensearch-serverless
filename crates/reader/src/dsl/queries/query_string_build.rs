//! `query_string` as a Lucene query: the classic grammar of the engine's
//! `lucene-queryparser`, with OpenSearch's `QueryStringQueryParser`
//! building each leaf (the engine calls `Leaves` where Java calls the
//! subclass's overrides), and `QueryStringQueryBuilder.doToQuery`, at
//! OpenSearch 3.10.0.
//!
//! A word or phrase is a `multi_match` of the query's `type` over the
//! fields; a wildcard, prefix, regexp, fuzzy term or range is one query a
//! field, combined as `dis_max` by the `type`'s tie breaker.

use std::sync::{Mutex, PoisonError};

use lucene_analysis::Analyzer;
use lucene_codecs::regexp::RegexpPattern;
use lucene_queryparser::classic::{
    Operator as ParserOperator, Overrides, QueryParser, QueryParserError,
};
use lucene_queryparser::{ParsedBoolean, ParsedQuery};
use lucene_search::query_visitor::Occur;
use lucene_search::{Clause, DisjunctionMaxQuery};

use super::Fuzziness;
use super::build;
use super::full_text::{MultiMatch, MultiType};
use super::full_text_build::{multi_match, resolve};
use super::leaf_common::{boosted, cx_is_string, edits_of, java_float, no_terms};
use super::query_string::QueryString;
use crate::dsl::query::Operator;
use crate::dsl::values::Value;
use crate::failure::Failure;
use crate::lucene_query::{
    QueryContext, apply_min_should_match, field_type, match_none, range, unsupported,
};
use crate::mapping::FieldType;

/// A hook's answer: the query, `None` for the `null` Java returns, or the
/// parse exception the grammar reports.
type Hooked = Result<Option<ParsedQuery>, QueryParserError>;

/// One leaf's query on one field, or the failure `rethrowUnlessLenient`
/// weighs.
type Leaf = Result<Clause, Failure>;

/// The class a runtime failure of a hook carries through the parser: the
/// failure itself waits in [`Leaves::failure`].
const STASHED: &str = "RuntimeException";

/// `QueryStringQueryBuilder.doToQuery`.
pub fn query_string(cx: &QueryContext<'_>, q: &QueryString) -> Result<Clause, Failure> {
    if q.default_field.is_some() && !q.fields.is_empty() {
        return Err(Failure::plain(
            "query_validation_exception",
            "Validation Failed: 1: [query_string] cannot use [fields] parameter in \
             conjunction with [default_field];",
        ));
    }
    let listed = listed_fields(q);
    let all_fields = listed.iter().any(|(f, _)| f == "*");
    let base: Vec<(String, f32)> = resolve(cx, &listed)?
        .into_iter()
        // ALLOC: one entry a resolved field, at most the mapping's fields.
        .map(|(f, b)| (f.to_owned(), b))
        // ALLOC: one entry a resolved field, at most the mapping's fields.
        .collect();
    let this_field = match (&q.default_field, q.fields.is_empty()) {
        (Some(f), _) => Some(f.clone()),
        // ALLOC: one string a query, at most the default field name.
        (None, true) => Some("*".to_owned()),
        (None, false) => None,
    };
    let leaves = Leaves {
        cx,
        q,
        base,
        this_field,
        lenient: q.lenient.unwrap_or(all_fields),
        failure: Mutex::new(None),
    };
    let analyzer = Analyzer::standard(None);
    let mut parser = match &leaves.this_field {
        Some(f) => QueryParser::new(f.as_str(), analyzer),
        None => QueryParser::multi_field(&[], analyzer),
    };
    parser.set_default_operator(if q.operator == Operator::And {
        ParserOperator::And
    } else {
        ParserOperator::Or
    });
    parser.set_phrase_slop(q.phrase_slop);
    let text = if q.escape {
        QueryParser::escape(&q.query)
    } else {
        q.query.clone()
    };
    // `QueryStringQueryParser.parse`: a blank query matches nothing.
    if text.trim_matches(|c: char| c <= ' ').is_empty() {
        return Ok(match_none());
    }
    let parsed = match parser.parse_with_overrides(&text, &leaves) {
        Ok(parsed) => parsed,
        Err(e) => return Err(leaves.failure_of(e, &q.query)),
    };
    let clause = parsed.to_clause(cx.max_doc);
    Ok(match clause {
        Clause::Boolean(b) => {
            // ALLOC: one box a boolean with its minimum should match.
            Clause::Boolean(Box::new(apply_min_should_match(
                *b,
                q.minimum_should_match.as_deref(),
            )?))
        }
        other => other,
    })
}

/// The fields the query names: `default_field`, else `fields`, else the
/// index's default (`*`).
fn listed_fields(q: &QueryString) -> Vec<(String, f32)> {
    match (&q.default_field, q.fields.is_empty()) {
        // ALLOC: one entry.
        (Some(f), _) => vec![(f.clone(), 1.0)],
        // ALLOC: one entry.
        (None, true) => vec![("*".to_owned(), 1.0)],
        (None, false) => q.fields.clone(),
    }
}

/// The overrides of `QueryStringQueryParser`.
struct Leaves<'a, 'c> {
    cx: &'a QueryContext<'c>,
    q: &'a QueryString,
    /// The fields a clause with no field of its own searches, resolved.
    base: Vec<(String, f32)>,
    /// The parser's own field: the default field, `*` for all fields, none
    /// for a list.
    this_field: Option<String>,
    lenient: bool,
    /// The failure a hook hit that is not a parse exception, which the
    /// grammar carries as [`STASHED`].
    failure: Mutex<Option<Failure>>,
}

impl Leaves<'_, '_> {
    /// Keeps `failure` for [`Self::failure_of`] and stops the parse.
    fn stash(&self, failure: Failure) -> QueryParserError {
        *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(failure);
        QueryParserError {
            class: STASHED,
            // ALLOC: empty; carries a stashed failure.
            message: String::new(),
        }
    }

    /// What the parse failed with, as OpenSearch reports it: a hook's own
    /// failure, a `ParseException` as a `QueryShardException`, anything
    /// else as the exception it is.
    fn failure_of(&self, e: QueryParserError, query: &str) -> Failure {
        if let Some(f) = self
            .failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            return f;
        }
        if e.is_parse_exception() {
            let inner = e
                .message
                // ALLOC: error path; one message.
                .strip_prefix(&format!("Cannot parse '{}': ", self.q.query))
                .map(str::to_owned);
            let cause = Failure::plain("parse_exception", e.message.clone());
            let cause = match inner {
                Some(inner) => cause.caused_by(Failure::plain("parse_exception", inner)),
                None => cause,
            };
            // ALLOC: error path; one message.
            return Failure::opensearch(
                "query_shard_exception",
                format!("Failed to parse query [{query}]"),
            )
            .caused_by(cause);
        }
        match e.class {
            "IllegalArgumentException" => Failure::plain("illegal_argument_exception", e.message),
            // ALLOC: error path; one message.
            other => Failure::plain("runtime_exception", format!("{other}: {}", e.message)),
        }
    }

    /// `extractMultiFields`: the fields a clause on `field` searches.
    fn extract(&self, field: Option<&str>) -> Result<Vec<(String, f32)>, QueryParserError> {
        match field {
            None => Ok(self.base.clone()),
            Some(f) => {
                // ALLOC: one entry.
                let listed = vec![(f.to_owned(), 1.0)];
                let resolved = resolve(self.cx, &listed).map_err(|e| self.stash(e))?;
                Ok(resolved
                    .into_iter()
                    // ALLOC: one entry a resolved field, at most the mapping's fields.
                    .map(|(n, b)| (n.to_owned(), b))
                    // ALLOC: one entry a resolved field, at most the mapping's fields.
                    .collect())
            }
        }
    }

    fn tie_breaker(&self) -> f32 {
        self.q
            .tie_breaker
            .unwrap_or_else(|| self.q.kind.tie_breaker())
    }

    /// A query of the parser's: `newUnmappedFieldQuery` when no field is
    /// left, else `make` on each field, combined as `dis_max`.
    fn per_field(
        &self,
        field: Option<&str>,
        mut make: impl FnMut(&str) -> Result<Leaf, QueryParserError>,
    ) -> Hooked {
        let fields = self.extract(field)?;
        if fields.is_empty() {
            return Ok(Some(ParsedQuery::Other(match_none())));
        }
        // ALLOC: one entry a resolved field, at most the mapping's fields.
        let mut disjuncts = Vec::with_capacity(fields.len());
        for (name, boost) in &fields {
            let query = match make(name)? {
                Ok(q) => q,
                Err(e) if self.lenient => {
                    drop(e);
                    match_none()
                }
                Err(e) => return Err(self.stash(e)),
            };
            disjuncts.push(boosted(query, *boost));
        }
        Ok(Some(ParsedQuery::Other(match disjuncts.len() {
            1 => disjuncts.swap_remove(0),
            // ALLOC: one box a dis_max over the fields.
            _ => Clause::DisjunctionMax(Box::new(DisjunctionMaxQuery::new(
                disjuncts,
                self.tie_breaker(),
            ))),
        })))
    }

    /// A word or phrase over the fields: `multi_match` of the query's type,
    /// `null` when the text analyzes to nothing in every field.
    fn matched(&self, field: Option<&str>, text: &str, kind: MultiType, slop: i32) -> Hooked {
        let fields = self.extract(field)?;
        if fields.is_empty() {
            return Ok(Some(ParsedQuery::Other(match_none())));
        }
        if slop < 0 {
            return Err(self.stash(Failure::plain(
                "illegal_argument_exception",
                "slop value cannot be negative",
            )));
        }
        let m = MultiMatch {
            // ALLOC: one string a clause, at most the query.
            query: Value::Str(text.to_owned()),
            // ALLOC: one entry a resolved field, at most the mapping's fields.
            fields: fields.clone(),
            kind,
            operator: self.q.operator,
            minimum_should_match: None,
            tie_breaker: self.q.tie_breaker,
            slop: u32::try_from(slop).unwrap_or(0),
            zero_terms_all: false,
            lenient: Some(self.lenient),
        };
        let clause = multi_match(self.cx, &m).map_err(|e| self.stash(e))?;
        let none = matches!(clause, Clause::MatchNoDocs(_))
            && no_terms(self.cx, fields.iter().map(|(f, _)| f.as_str()), text, None);
        Ok((!none).then_some(ParsedQuery::Other(clause)))
    }

    /// The analyzer's `normalize` of a term for a field: a text field's
    /// lowercasing, a keyword field's none.
    fn normalized(&self, field: &str, text: &str) -> Result<String, Failure> {
        match field_type(self.cx, field)? {
            Some(FieldType::Text)
                if self
                    .cx
                    .mapping
                    .get(field)
                    .is_some_and(|f| f.analyzer.is_some()) =>
            {
                // ALLOC: error path; one message.
                Err(crate::lucene_query::unsupported(&format!(
                    "a prefix, wildcard or fuzzy term on [{field}], which a pinned analyzer analyses"
                )))
            }
            Some(FieldType::Text) => {
                let bytes = self
                    .cx
                    .analyzer
                    .normalize(field, text)
                    // ALLOC: error path; one message.
                    .map_err(|e| Failure::plain("illegal_argument_exception", e.to_string()))?;
                Ok(String::from_utf8_lossy(&bytes).into_owned())
            }
            // ALLOC: one string a term, at most the query.
            _ => Ok(text.to_owned()),
        }
    }

    /// A fuzzy term of `edits` on every field.
    fn fuzzy(&self, field: Option<&str>, term: &str, edits: u8) -> Hooked {
        self.per_field(field, |f| {
            Ok((|| {
                let term = self.normalized(f, term)?;
                build::fuzzy(
                    self.cx,
                    f,
                    &Value::Str(term),
                    Fuzziness::Edits(edits),
                    self.q.fuzzy_prefix_length,
                    self.q.fuzzy_max_expansions,
                    self.q.fuzzy_transpositions,
                )
            })())
        })
    }

    /// `getFieldQuery`'s `>`, `>=`, `<` and `<=`: on a clause with a field,
    /// a word that starts with one is a range open at the other end.
    fn comparison(&self, field: Option<&str>, first: char, text: &str) -> Option<Hooked> {
        if text.len() < 2 || !matches!(first, '>' | '<') {
            return None;
        }
        let equals = text.len() > 2 && text.as_bytes().get(1) == Some(&b'=');
        let rest = text.get(if equals { 2 } else { 1 }..)?;
        Some(if first == '>' {
            self.range_query(field, Some(rest), None, equals, true)
        } else {
            self.range_query(field, None, Some(rest), true, equals)
        })
    }

    fn allow_leading_wildcard(&self) -> bool {
        self.q.allow_leading_wildcard.unwrap_or(true)
    }
}

impl Overrides for Leaves<'_, '_> {
    fn field_query(&self, field: Option<&str>, text: &str, quoted: bool) -> Hooked {
        if let (false, Some(_), Some(first)) = (quoted, field, text.chars().next())
            && let Some(range) = self.comparison(field, first, text)
        {
            return range;
        }
        let kind = if quoted {
            MultiType::Phrase
        } else {
            self.q.kind
        };
        self.matched(field, text, kind, self.q.phrase_slop)
    }

    fn field_query_slop(&self, field: Option<&str>, text: &str, slop: i32) -> Hooked {
        self.matched(field, text, MultiType::Phrase, slop)
    }

    fn range_query(
        &self,
        field: Option<&str>,
        part1: Option<&str>,
        part2: Option<&str>,
        start_inclusive: bool,
        end_inclusive: bool,
    ) -> Hooked {
        // ALLOC: one string a range bound, at most the query.
        let from = part1.map(|s| Value::Str(s.to_owned()));
        // ALLOC: one string a range bound, at most the query.
        let to = part2.map(|s| Value::Str(s.to_owned()));
        self.per_field(field, |f| {
            match field_type(self.cx, f) {
                Ok(Some(FieldType::Text | FieldType::Keyword)) => {
                    // ALLOC: error path; one message.
                    return Err(self.stash(unsupported(&format!(
                        "a range query on the string field [{f}]"
                    ))));
                }
                Ok(_) => {}
                Err(e) => return Err(self.stash(e)),
            }
            // An open end is unbounded whatever its flag says.
            Ok(range(
                self.cx,
                f,
                from.as_ref(),
                to.as_ref(),
                start_inclusive || from.is_none(),
                end_inclusive || to.is_none(),
            ))
        })
    }

    fn wildcard_query(&self, field: Option<&str>, term: &str) -> Hooked {
        let actual = field.or(self.this_field.as_deref());
        if term == "*"
            && let Some(actual) = actual
        {
            return Ok(Some(ParsedQuery::Other(if actual == "*" {
                Clause::MatchAllDocs(lucene_search::MatchAllDocsQuery::new(self.cx.max_doc))
            } else {
                build::exists(self.cx, actual).map_err(|e| self.stash(e))?
            })));
        }
        self.per_field(field, |f| {
            if term == "*" {
                return Ok(build::exists(self.cx, f));
            }
            if !self.allow_leading_wildcard() && term.starts_with(['*', '?']) {
                return Err(QueryParserError::parse_exception(
                    "'*' or '?' not allowed as first character in WildcardQuery",
                ));
            }
            Ok((|| {
                let pattern = self.normalized(f, term)?;
                build::wildcard_clause(self.cx, f, &pattern)
            })())
        })
    }

    fn regexp_query(&self, field: Option<&str>, term: &str) -> Hooked {
        self.per_field(field, |f| {
            Ok((|| {
                let pattern = self.normalized(f, term)?;
                // Java builds the automaton here, so a bad expression fails
                // the query rather than the search.
                if cx_is_string(self.cx, f) {
                    RegexpPattern::parse(&pattern).map_err(|e| {
                        // ALLOC: error path; one message.
                        Failure::plain("illegal_argument_exception", e.to_string())
                    })?;
                }
                build::regexp_clause(self.cx, f, &pattern)
            })())
        })
    }

    fn prefix_query(&self, field: Option<&str>, term: &str) -> Hooked {
        self.per_field(field, |f| {
            if !self.allow_leading_wildcard() && term.starts_with('*') {
                return Err(QueryParserError::parse_exception(
                    "'*' not allowed as first character in PrefixQuery",
                ));
            }
            Ok((|| {
                let term = self.normalized(f, term)?;
                build::prefix_clause(self.cx, f, &term)
            })())
        })
    }

    fn fuzzy_query(&self, field: Option<&str>, term: &str, min_similarity: f32) -> Hooked {
        self.fuzzy(field, term, edits_of(min_similarity))
    }

    fn bare_fuzzy(&self, field: Option<&str>, fuzzy_slop: &str, term_image: &str) -> Hooked {
        let digits = fuzzy_slop.strip_prefix('~').unwrap_or(fuzzy_slop);
        let edits = if digits.is_empty() {
            self.q.fuzziness.as_distance(term_image)
        } else if let Some(f) = java_float(digits) {
            edits_of(f)
        } else {
            return Err(self.stash(Failure::plain(
                "number_format_exception",
                // ALLOC: error path; one message.
                format!("For input string: \"{}\"", digits.to_uppercase()),
            )));
        };
        self.fuzzy(field, term_image, edits)
    }

    fn boolean_query(&self, clauses: Vec<(Occur, ParsedQuery)>) -> Hooked {
        if clauses.is_empty() {
            return Ok(None);
        }
        let mut b = ParsedBoolean {
            clauses,
            minimum_should_match: 0,
        };
        if b.clauses.iter().all(|(o, _)| *o == Occur::MustNot) {
            b.clauses.push((Occur::Filter, ParsedQuery::MatchAll));
        }
        Ok(Some(ParsedQuery::Boolean(b)))
    }
}

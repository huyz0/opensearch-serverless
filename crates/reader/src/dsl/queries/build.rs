//! This module's queries as Lucene queries: each builder's `doRewrite` (an
//! unmapped field, or no values, matches nothing) and `doToQuery`, with
//! the field types' `termsQuery`, `prefixQuery`, `wildcardQuery`,
//! `regexpQuery`, `fuzzyQuery` and `existsQuery`
//! (server/src/main/java/org/opensearch/index/mapper/), and
//! `ExistsQueryBuilder.newFilter`, at OpenSearch 3.10.0.
//!
//! The multi-term queries are constant-scored, as their default rewrite
//! (`CONSTANT_SCORE_BLENDED_REWRITE`) scores them; a keyword field's
//! index-or-doc-values form is searched by its index alone, which matches
//! the same documents.

use lucene_search::extended_query::{ExtendedQuery, PointInSetQuery};
use lucene_search::function::FunctionScoreQuery;
use lucene_search::query::FieldExistsQuery;
use lucene_search::{
    BooleanQuery, Clause, ConstantScoreQuery, DisjunctionMaxQuery, FuzzyQuery, PrefixQuery,
    RegexpQuery, TermInSetQuery, TermQuery, WildcardQuery,
};

use crate::dsl::bool_query::MAX_CLAUSES;
use crate::dsl::queries::{Fuzziness, More, full_text_build, uid};
use crate::dsl::values::Value;
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, build, field_type, match_none, term, unsupported};
use crate::mapping::FieldType;
use crate::numbers;
use crate::numbers_decimal as decimal;

/// `index.max_terms_count`'s default: the most values one `terms` query
/// may hold.
pub const MAX_TERMS_COUNT: usize = 65_536;

/// `index.max_regex_length`'s default: the most characters a `regexp`
/// query's pattern may hold.
pub const MAX_REGEX_LENGTH: usize = 1000;

/// The query `m` builds, before its boost.
pub fn to_clause(m: &More, cx: &QueryContext<'_>) -> Result<Clause, Failure> {
    match m {
        More::Terms { field, values, .. } => terms(cx, field, values),
        More::Prefix { field, value, .. } => {
            not_on_a_pin(cx, "prefix", field)?;
            string_query(cx, "prefix", field, || {
                Clause::Prefix(PrefixQuery::new(field.as_str(), value.as_bytes()))
            })
        }
        More::Wildcard { field, value, .. } => {
            not_on_a_pin(cx, "wildcard", field)?;
            wildcard_clause(cx, field, value)
        }
        More::Regexp { field, value, .. } => {
            not_on_a_pin(cx, "regexp", field)?;
            regexp_clause(cx, field, value)
        }
        More::Fuzzy {
            field,
            value,
            fuzziness,
            prefix_length,
            max_expansions,
            transpositions,
            ..
        } => {
            not_on_a_pin(cx, "fuzzy", field)?;
            fuzzy(
                cx,
                field,
                value,
                *fuzziness,
                *prefix_length,
                *max_expansions,
                *transpositions,
            )
        }
        More::CaseInsensitive {
            kind, field, value, ..
        } => super::case_insensitive::build(cx, *kind, field, value),
        More::Exists { field, .. } => exists(cx, field),
        More::Ids { values, .. } => ids(values),
        More::ConstantScore { filter, .. } => Ok(constant(build(filter, cx)?)),
        More::DisMax {
            queries,
            tie_breaker,
            ..
        } => {
            if queries.is_empty() {
                return Ok(match_none());
            }
            let clauses = queries
                .iter()
                .map(|q| build(q, cx))
                // ALLOC: one entry a disjunct, at most MAX_CLAUSES.
                .collect::<Result<Vec<_>, _>>()?;
            // ALLOC: one box a dis_max query.
            Ok(Clause::DisjunctionMax(Box::new(DisjunctionMaxQuery::new(
                clauses,
                *tie_breaker,
            ))))
        }
        More::Boosting {
            positive,
            negative,
            negative_boost,
            ..
        } => {
            let scored = FunctionScoreQuery::boost_by_query(
                build(positive, cx)?,
                build(negative, cx)?,
                *negative_boost,
            );
            // ALLOC: one box a boosting query.
            Ok(Clause::Extended(Box::new(ExtendedQuery::FunctionScore(
                scored,
            ))))
        }
        More::MatchNone { .. } => Ok(match_none()),
        More::MatchPhrase {
            field,
            query,
            slop,
            zero_terms_all,
            ..
        } => full_text_build::phrase(cx, field, query, *slop, *zero_terms_all),
        More::MatchBoolPrefix(m, _) => super::bool_prefix::build(cx, m),
        More::MatchPhrasePrefix(m, _) => super::phrase_prefix::build(cx, m),
        More::FuzzyMatch(m, _) => full_text_build::fuzzy_match(cx, m),
        More::MultiMatch(m, _) => full_text_build::multi_match(cx, m),
        More::SimpleQueryString(s, _) => super::simple_string_build::simple_query_string(cx, s),
        More::QueryString(q, _) => super::query_string_build::query_string(cx, q),
        More::FunctionScore(q, _) => super::function_score_build::function_score(cx, q),
        More::Script(q, _) => super::script_query::build(cx, q),
        More::Knn(q) if matches!(q.mode, super::knn::Mode::TopK(_)) => cx
            .knn
            .and_then(|r| r.get(q))
            .ok_or_else(|| unsupported("the [knn] query here")),
        More::Knn(q) => crate::knn::radial(q, cx),
        More::Nested(q, _) => super::nested_build::nested(q, cx),
        More::Sltr(_) => Err(unsupported(
            "an [sltr] query anywhere but as the rescore_query of a rescore",
        )),
    }
}

/// A query that works on the terms of a field is refused on a field whose
/// terms a pinned module made, which no query here can expand.
fn not_on_a_pin(cx: &QueryContext<'_>, kind: &str, field: &str) -> Result<(), Failure> {
    match cx.mapping.get(field).and_then(|f| f.analyzer.as_deref()) {
        // ALLOC: error path; one message.
        Some(pin) => Err(unsupported(&format!(
            "a [{kind}] query on field [{field}], which pins the analyzer [{pin}]"
        ))),
        None => Ok(()),
    }
}

fn constant(inner: Clause) -> Clause {
    // ALLOC: one box a constant-score query.
    Clause::ConstantScore(Box::new(ConstantScoreQuery::new(inner, 1.0)))
}

/// The type name OpenSearch's messages give a field type.
pub(super) fn type_name(ty: &FieldType) -> &str {
    match ty {
        FieldType::Text => "text",
        FieldType::Keyword => "keyword",
        FieldType::Int { name, .. } => name,
        FieldType::Long => "long",
        FieldType::Date => "date",
        FieldType::Float => "float",
        FieldType::Double => "double",
        FieldType::Unsupported(what) => what,
    }
}

/// `prefixQuery` on one field, as `newPrefixQuery` builds it per field.
pub(crate) fn prefix_clause(
    cx: &QueryContext<'_>,
    field: &str,
    term: &str,
) -> Result<Clause, Failure> {
    string_query(cx, "prefix", field, || {
        Clause::Prefix(PrefixQuery::new(field, term.as_bytes()))
    })
}

/// `wildcardQuery` on one field.
pub(crate) fn wildcard_clause(
    cx: &QueryContext<'_>,
    field: &str,
    pattern: &str,
) -> Result<Clause, Failure> {
    string_query(cx, "wildcard", field, || {
        Clause::Wildcard(WildcardQuery::new(field, pattern.as_bytes()))
    })
}

/// `regexpQuery` on one field, refused past `index.max_regex_length`.
pub(crate) fn regexp_clause(
    cx: &QueryContext<'_>,
    field: &str,
    value: &str,
) -> Result<Clause, Failure> {
    if value.chars().count() > MAX_REGEX_LENGTH && cx.mapping.get(field).is_some() {
        return Err(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "The length of regex [{}] used in the Regexp Query request has exceeded the \
                 allowed maximum of [{MAX_REGEX_LENGTH}]. This maximum can be set by \
                 changing the [index.max_regex_length] index level setting.",
                value.chars().count()
            ),
        ));
    }
    string_query(cx, "regexp", field, || {
        Clause::Regexp(RegexpQuery::new(field, value))
    })
}

/// `prefixQuery`, `wildcardQuery` and `regexpQuery`: string fields only;
/// any other type is a `QueryShardException`.
pub(crate) fn string_query(
    cx: &QueryContext<'_>,
    kind: &str,
    field: &str,
    make: impl FnOnce() -> Clause,
) -> Result<Clause, Failure> {
    let Some(ty) = field_type(cx, field)? else {
        return Ok(match_none());
    };
    match ty {
        FieldType::Text | FieldType::Keyword => Ok(make()),
        other => Err(Failure::opensearch(
            "query_shard_exception",
            // ALLOC: error path; one message.
            format!(
                "Can only use {kind} queries on keyword and text fields - not on [{field}] which \
                 is of type [{}]",
                type_name(other)
            ),
        )),
    }
}

/// `MappedFieldType.termsQuery` for each type.
fn terms(cx: &QueryContext<'_>, field: &str, values: &[Value]) -> Result<Clause, Failure> {
    let Some(ty) = field_type(cx, field)? else {
        return Ok(match_none());
    };
    if values.is_empty() {
        return Ok(match_none());
    }
    if values.len() > MAX_TERMS_COUNT {
        return Err(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "The number of terms [{}] used in the Terms Query request has exceeded the \
                 allowed maximum of [{MAX_TERMS_COUNT}]. This maximum can be set by changing \
                 the [index.max_terms_count] index level setting.",
                values.len()
            ),
        ));
    }
    let set = |q: lucene_search::Result<PointInSetQuery>| -> Result<Clause, Failure> {
        let q = q.map_err(|e| {
            // ALLOC: error path; one message.
            Failure::plain("illegal_argument_exception", e.to_string())
        })?;
        // ALLOC: one box a point set query.
        Ok(Clause::Extended(Box::new(ExtendedQuery::PointInSet(q))))
    };
    match ty {
        FieldType::Text | FieldType::Keyword => Ok(Clause::TermInSet(TermInSetQuery::new(
            field,
            // ALLOC: one term a value, bounded by the body's recording.
            values.iter().map(|v| v.text().as_bytes().to_vec()),
        ))),
        FieldType::Int { .. } => {
            // ALLOC: one entry a value, bounded by the body's recording.
            let mut ints = Vec::with_capacity(values.len());
            for v in values {
                if !numbers::has_decimal_part(v)? {
                    ints.push(numbers::parse_int(v)?);
                }
            }
            if ints.is_empty() {
                return Ok(match_none());
            }
            set(PointInSetQuery::int_set(field, &ints))
        }
        FieldType::Long => {
            // ALLOC: one entry a value, bounded by the body's recording.
            let mut longs = Vec::with_capacity(values.len());
            for v in values {
                if !numbers::has_decimal_part(v)? {
                    longs.push(numbers::parse_long(v)?);
                }
            }
            if longs.is_empty() {
                return Ok(match_none());
            }
            set(PointInSetQuery::long_set(field, &longs))
        }
        FieldType::Date => {
            if values.len() > MAX_CLAUSES {
                return Err(Failure::plain(
                    "too_many_clauses",
                    // ALLOC: error path; one message.
                    format!("maxClauseCount is set to {MAX_CLAUSES}"),
                ));
            }
            let mut q = BooleanQuery::default();
            for v in values {
                q.should.push(term(cx, field, v)?);
            }
            // ALLOC: one box a bool of a date field's terms.
            Ok(constant(Clause::Boolean(Box::new(q))))
        }
        FieldType::Double => {
            // ALLOC: one entry a value, bounded by the body's recording.
            let mut doubles = Vec::with_capacity(values.len());
            for v in values {
                doubles.push(decimal::parse_double(v)?);
            }
            set(decimal::double_set(field, &doubles))
        }
        FieldType::Float => {
            // ALLOC: one entry a value, bounded by the body's recording.
            let mut floats = Vec::with_capacity(values.len());
            for v in values {
                floats.push(decimal::parse_float(v)?);
            }
            set(decimal::float_set(field, &floats))
        }
        FieldType::Unsupported(_) => Err(unsupported("this field type")),
    }
}

/// `StringFieldType.fuzzyQuery` and `FuzzyQuery`'s constructor checks.
pub(crate) fn fuzzy(
    cx: &QueryContext<'_>,
    field: &str,
    value: &Value,
    fuzziness: Fuzziness,
    prefix_length: i32,
    max_expansions: i32,
    transpositions: bool,
) -> Result<Clause, Failure> {
    let Some(ty) = field_type(cx, field)? else {
        return Ok(match_none());
    };
    if !matches!(ty, FieldType::Text | FieldType::Keyword) {
        return Err(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "Can only use fuzzy queries on keyword and text fields - not on [{field}] which \
                 is of type [{}]",
                type_name(ty)
            ),
        ));
    }
    let illegal = |reason: &str| Failure::plain("illegal_argument_exception", reason);
    let Ok(prefix_length) = usize::try_from(prefix_length) else {
        return Err(illegal("prefixLength cannot be negative."));
    };
    // `TopTermsRewrite` keeps at most `maxClauseCount` terms.
    let max_expansions = match usize::try_from(max_expansions) {
        Ok(n) if n > 0 => n.min(MAX_CLAUSES),
        _ => return Err(illegal("maxExpansions must be positive.")),
    };
    let text = value.text();
    Ok(Clause::Fuzzy(FuzzyQuery {
        // ALLOC: the field name, once a fuzzy query.
        field: field.to_owned(),
        // ALLOC: the term's bytes, once a fuzzy query.
        term: text.as_bytes().to_vec(),
        max_edits: fuzziness.as_distance(text),
        prefix_length,
        transpositions,
        max_expansions,
    }))
}

/// `Regex.simpleMatch` for one `*`-pattern.
pub(crate) fn simple_match(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let mut parts = parts.peekable();
    while let Some(piece) = parts.next() {
        if parts.peek().is_none() {
            return rest.len() >= piece.len() && rest.ends_with(piece);
        }
        match rest.find(piece) {
            Some(at) => rest = rest.get(at.saturating_add(piece.len())..).unwrap_or(""),
            None => return false,
        }
    }
    rest.is_empty()
}

/// `MappedFieldType.existsQuery`: doc values, else norms (a text field),
/// else the `_field_names` term.
fn exists_one(cx: &QueryContext<'_>, field: &str) -> Result<Clause, Failure> {
    let Some(f) = cx.mapping.get(field) else {
        return Ok(match_none());
    };
    if let FieldType::Unsupported(what) = &f.ty {
        // ALLOC: error path; one message.
        return Err(unsupported(&format!("an exists query on {what} [{field}]")));
    }
    let inner = if f.doc_values || f.ty == FieldType::Text {
        Clause::Exists(FieldExistsQuery::new(field))
    } else {
        Clause::Term(TermQuery::new("_field_names", field.as_bytes()))
    };
    Ok(constant(inner))
}

/// Whether `name` is a field under the object `object`.
fn is_under(object: &str, name: &str) -> bool {
    // A mapped name never ends in a dot, so a dot after the object's name
    // is followed by a sub-field's.
    name.strip_prefix(object)
        .is_some_and(|rest| rest.starts_with('.'))
}

/// `ExistsQueryBuilder.newFilter`: the fields the pattern names (an object
/// names its sub-fields), none matching nothing.
pub(crate) fn exists(cx: &QueryContext<'_>, pattern: &str) -> Result<Clause, Failure> {
    let mut fields: Vec<&str> = if pattern.contains('*') {
        let matching = cx.mapping.names().filter(|n| simple_match(pattern, n));
        // ALLOC: one name a field the pattern matches, at most MAX_FIELDS.
        matching.collect()
    } else if cx.mapping.get(pattern).is_some() {
        // ALLOC: one name.
        vec![pattern]
    } else {
        // ALLOC: empty.
        Vec::new()
    };
    if fields.is_empty() && !pattern.contains('*') {
        // An object: every field under it.
        let under = cx.mapping.names().filter(|n| is_under(pattern, n));
        // ALLOC: one name a field under the object, at most MAX_FIELDS.
        fields = under.collect();
        if fields.is_empty() {
            return Ok(match_none());
        }
        return several(cx, &fields);
    }
    match fields.as_slice() {
        [] => Ok(match_none()),
        [one] => exists_one(cx, one),
        many => several(cx, many),
    }
}

fn several(cx: &QueryContext<'_>, fields: &[&str]) -> Result<Clause, Failure> {
    let mut q = BooleanQuery::default();
    for f in fields {
        q.should.push(exists_one(cx, f)?);
    }
    // ALLOC: one box a bool of exists queries.
    Ok(constant(Clause::Boolean(Box::new(q))))
}

/// `IdFieldType.termsQuery` over the encoded ids.
fn ids(values: &[String]) -> Result<Clause, Failure> {
    if values.is_empty() {
        return Ok(match_none());
    }
    // ALLOC: one entry an id, bounded by the body's recording.
    let mut encoded = Vec::with_capacity(values.len());
    for v in values {
        let Some(bytes) = uid::encode_id(v) else {
            return Err(Failure::plain(
                "illegal_argument_exception",
                "Ids can't be empty",
            ));
        };
        encoded.push(bytes);
    }
    Ok(Clause::TermInSet(TermInSetQuery::new("_id", encoded)))
}

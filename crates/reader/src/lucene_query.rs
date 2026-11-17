//! A parsed query as a Lucene query over an index's mapping.
//!
//! Ported from each builder's `doToQuery` and `AbstractQueryBuilder.toQuery`
//! (server/src/main/java/org/opensearch/index/query/), `MatchQuery.parse`
//! (server/src/main/java/org/opensearch/index/search/MatchQuery.java),
//! `Queries.calculateMinShouldMatch`, `#applyMinimumShouldMatch` and
//! `#fixNegativeQueryIfNeeded`
//! (server/src/main/java/org/opensearch/common/lucene/search/Queries.java),
//! and the field types' `termQuery` and `rangeQuery` (`TextFieldMapper`,
//! `KeywordFieldMapper`, `NumberFieldMapper`, `DateFieldMapper`), at
//! OpenSearch 3.10.0. An unmapped field matches nothing. A failure is the
//! exception Java throws; [`shard_failure`] wraps it as the shard reports it.

use lucene_analysis::Analyzer;
use lucene_search::extended_query::{ExtendedQuery, PointRangeQuery};
use lucene_search::query_builder::QueryBuilder;
use lucene_search::query_visitor::Occur;
use lucene_search::{
    BooleanQuery, BoostQuery, Clause, ConstantScoreQuery, MatchAllDocsQuery, MatchNoDocsQuery,
    TermQuery,
};

use crate::date;
use crate::dsl::{BoolQuery, Operator, Query, Value};
use crate::failure::{Failure, Meta};
use crate::mapping::{FieldType, Mapping};
use crate::numbers;

/// What a query is built against.
pub struct QueryContext<'a> {
    /// The index's fields.
    pub mapping: &'a Mapping,
    /// The analyzer of `text` fields: the standard analyzer.
    pub analyzer: &'a Analyzer,
    /// The reader's `maxDoc`, which a match-all query covers.
    pub max_doc: i32,
}

/// `QueryShardContext.toQuery`'s wrapping: an exception that is not an
/// OpenSearch exception becomes a `query_shard_exception` naming the index.
pub fn shard_failure(f: Failure, index: &str, uuid: &str) -> Failure {
    if f.opensearch {
        // A `QueryShardException` names the index it was thrown on.
        if f.kind == "query_shard_exception" && f.meta.is_empty() {
            return f
                // ALLOC: error path; the index name.
                .with_meta("index", Meta::Str(index.to_owned()))
                // ALLOC: error path; the index uuid.
                .with_meta("index_uuid", Meta::Str(uuid.to_owned()));
        }
        return f;
    }
    // ALLOC: error path; one message.
    let reason = format!("failed to create query: {}", f.reason);
    Failure::opensearch("query_shard_exception", reason)
        // ALLOC: error path; the index name.
        .with_meta("index", Meta::Str(index.to_owned()))
        // ALLOC: error path; the index uuid.
        .with_meta("index_uuid", Meta::Str(uuid.to_owned()))
        .caused_by(f)
}

/// The query to search with: a top-level clause that is not boolean is the
/// one `MUST` clause of one, which scores the same.
pub fn to_lucene(query: &Query, cx: &QueryContext<'_>) -> Result<BooleanQuery, Failure> {
    Ok(match build(query, cx)? {
        Clause::Boolean(b) => *b,
        other => BooleanQuery {
            // ALLOC: one clause.
            must: vec![other],
            ..BooleanQuery::default()
        },
    })
}

/// `AbstractQueryBuilder.toQuery`: the builder's query, boosted unless the
/// boost is 1 or nothing can match.
pub(crate) fn build(query: &Query, cx: &QueryContext<'_>) -> Result<Clause, Failure> {
    let (clause, boost) = match query {
        Query::MatchAll { boost } => (match_all(cx), *boost),
        Query::Term {
            field,
            value,
            boost,
        } => (term(cx, field, value)?, *boost),
        Query::Match {
            field,
            query,
            operator,
            minimum_should_match,
            zero_terms_all,
            boost,
        } => (
            match_query(
                cx,
                field,
                query,
                *operator,
                minimum_should_match.as_deref(),
                *zero_terms_all,
            )?,
            *boost,
        ),
        Query::Range {
            field,
            from,
            to,
            include_lower,
            include_upper,
            boost,
        } => (
            range(
                cx,
                field,
                from.as_ref(),
                to.as_ref(),
                *include_lower,
                *include_upper,
            )?,
            *boost,
        ),
        Query::Bool(b) => (bool_query(cx, b)?, b.boost),
        Query::More(m) => (crate::dsl::queries::build::to_clause(m, cx)?, m.boost()),
    };
    if boost == 1.0 || matches!(clause, Clause::MatchNoDocs(_)) {
        return Ok(clause);
    }
    // ALLOC: one box a boosted query, at most one a parsed query.
    Ok(Clause::Boost(Box::new(BoostQuery::new(clause, boost))))
}

pub(crate) fn match_all(cx: &QueryContext<'_>) -> Clause {
    Clause::MatchAllDocs(MatchAllDocsQuery::new(cx.max_doc))
}

pub(crate) fn match_none() -> Clause {
    Clause::MatchNoDocs(MatchNoDocsQuery::new())
}

pub(crate) fn unsupported(what: &str) -> Failure {
    Failure::plain(
        "illegal_argument_exception",
        // ALLOC: error path; one message.
        format!("{what} is not supported by this node yet"),
    )
}

/// The field's type, refusing one this reader does not search.
pub(crate) fn field_type<'m>(
    cx: &QueryContext<'m>,
    field: &str,
) -> Result<Option<&'m FieldType>, Failure> {
    let Some(f) = cx.mapping.get(field) else {
        return Ok(None);
    };
    if let FieldType::Unsupported(what) = &f.ty {
        // ALLOC: error path; one message.
        return Err(unsupported(&format!("searching {what} [{field}]")));
    }
    if !f.indexed {
        // ALLOC: error path; one message.
        return Err(unsupported(&format!(
            "searching the field [{field}], which is not indexed,"
        )));
    }
    Ok(Some(&f.ty))
}

/// `MappedFieldType.termQuery` for each type of the subset.
pub(crate) fn term(cx: &QueryContext<'_>, field: &str, value: &Value) -> Result<Clause, Failure> {
    let Some(ty) = field_type(cx, field)? else {
        return Ok(match_none());
    };
    // ALLOC: the field name and the term's bytes, once a term query.
    let term_query = || TermQuery::new(field, value.text().as_bytes().to_vec());
    Ok(match ty {
        FieldType::Text => Clause::Term(term_query()),
        // ALLOC: one box a keyword term query.
        FieldType::Keyword => {
            Clause::ConstantScore(Box::new(ConstantScoreQuery::new(term_query(), 1.0)))
        }
        FieldType::Int { .. } => {
            if numbers::has_decimal_part(value)? {
                return Ok(match_none());
            }
            let v = numbers::parse_int(value)?;
            point(PointRangeQuery::int_range(field, &[v], &[v]))?
        }
        FieldType::Long => {
            if numbers::has_decimal_part(value)? {
                return Ok(match_none());
            }
            let v = numbers::parse_long(value)?;
            point(PointRangeQuery::long_range(field, &[v], &[v]))?
        }
        FieldType::Date => date_range(field, Some(value), Some(value), true, true)?,
        FieldType::Unsupported(_) => return Err(unsupported("this field type")),
    })
}

pub(crate) fn point(q: lucene_search::Result<PointRangeQuery>) -> Result<Clause, Failure> {
    let q = q.map_err(|e| {
        // ALLOC: error path; one message.
        Failure::plain("illegal_argument_exception", e.to_string())
    })?;
    // ALLOC: one box a point query.
    Ok(Clause::Extended(Box::new(ExtendedQuery::PointRange(q))))
}

/// `DateFieldType.rangeQuery` over the default format.
fn date_range(
    field: &str,
    from: Option<&Value>,
    to: Option<&Value>,
    include_lower: bool,
    include_upper: bool,
) -> Result<Clause, Failure> {
    let lower = match from {
        Some(v) => {
            date::lower_bound(v.text(), include_lower).map_err(|e| e.into_failure(v.text()))?
        }
        None => i64::MIN,
    };
    let upper = match to {
        Some(v) => {
            date::upper_bound(v.text(), include_upper).map_err(|e| e.into_failure(v.text()))?
        }
        None => i64::MAX,
    };
    if lower > upper {
        return Ok(match_none());
    }
    point(PointRangeQuery::long_range(field, &[lower], &[upper]))
}

/// `RangeQueryBuilder.doToQuery` for numeric and date fields.
fn range(
    cx: &QueryContext<'_>,
    field: &str,
    from: Option<&Value>,
    to: Option<&Value>,
    include_lower: bool,
    include_upper: bool,
) -> Result<Clause, Failure> {
    let Some(ty) = field_type(cx, field)? else {
        return Ok(match_none());
    };
    match ty {
        FieldType::Int { .. } => {
            match numbers::int_range(from, to, include_lower, include_upper)? {
                Some((l, u)) => point(PointRangeQuery::int_range(field, &[l], &[u])),
                None => Ok(match_none()),
            }
        }
        FieldType::Long => match numbers::long_range(from, to, include_lower, include_upper)? {
            Some((l, u)) => point(PointRangeQuery::long_range(field, &[l], &[u])),
            None => Ok(match_none()),
        },
        FieldType::Date => date_range(field, from, to, include_lower, include_upper),
        FieldType::Text | FieldType::Keyword | FieldType::Unsupported(_) => {
            // ALLOC: error path; one message.
            Err(unsupported(&format!(
                "a range query on the string field [{field}]"
            )))
        }
    }
}

/// `MatchQuery.parse` for the boolean type: a text field's terms combined
/// by the operator, any other field's term query.
pub(crate) fn match_query(
    cx: &QueryContext<'_>,
    field: &str,
    value: &Value,
    operator: Operator,
    minimum_should_match: Option<&str>,
    zero_terms_all: bool,
) -> Result<Clause, Failure> {
    let Some(ty) = field_type(cx, field)? else {
        return Ok(match_none());
    };
    if *ty != FieldType::Text {
        return term(cx, field, value);
    }
    let occur = match operator {
        Operator::Or => Occur::Should,
        Operator::And => Occur::Must,
    };
    let built = QueryBuilder::new(cx.analyzer)
        .create_boolean_query_with(field, value.text(), occur)
        .map_err(|e| {
            // ALLOC: error path; one message.
            Failure::plain("illegal_argument_exception", e.to_string())
        })?;
    let Some(clause) = built else {
        return Ok(if zero_terms_all {
            match_all(cx)
        } else {
            match_none()
        });
    };
    match clause {
        // ALLOC: one box a match query's terms.
        Clause::Boolean(b) => Ok(Clause::Boolean(Box::new(apply_min_should_match(
            *b,
            minimum_should_match,
        )?))),
        other => Ok(other),
    }
}

/// `BoolQueryBuilder.doToQuery`: the clauses in the order `MUST`,
/// `MUST_NOT`, `SHOULD`, `FILTER`; none is match-all; then the minimum
/// should match, and a match-all filter beside a purely negative query.
fn bool_query(cx: &QueryContext<'_>, b: &BoolQuery) -> Result<Clause, Failure> {
    let mut q = BooleanQuery::default();
    let each = |clauses: &[Query]| -> Result<Vec<Clause>, Failure> {
        // ALLOC: one entry a clause, at most MAX_CLAUSES a bool query.
        clauses.iter().map(|c| build(c, cx)).collect()
    };
    q.must = each(&b.must)?;
    q.must_not = each(&b.must_not)?;
    q.should = each(&b.should)?;
    q.filter = each(&b.filter)?;
    let count = q
        .must
        .len()
        .saturating_add(q.must_not.len())
        .saturating_add(q.should.len())
        .saturating_add(q.filter.len());
    if count == 0 {
        return Ok(match_all(cx));
    }
    let mut q = apply_min_should_match(q, b.minimum_should_match.as_deref())?;
    if b.adjust_pure_negative && q.must_not.len() == count {
        q.filter.push(match_all(cx));
    }
    // ALLOC: one box a bool query.
    Ok(Clause::Boolean(Box::new(q)))
}

/// `Queries.applyMinimumShouldMatch`: the spec over the `SHOULD` clauses,
/// set when it asks for at least one.
fn apply_min_should_match(
    mut q: BooleanQuery,
    spec: Option<&str>,
) -> Result<BooleanQuery, Failure> {
    let Some(spec) = spec else {
        return Ok(q);
    };
    let optional = i32::try_from(q.should.len()).unwrap_or(i32::MAX);
    let msm = calculate_min_should_match(optional, spec)?;
    if msm > 0 {
        q.minimum_should_match = usize::try_from(msm).unwrap_or(0);
    }
    Ok(q)
}

/// `Integer.parseInt`.
fn parse_int(text: &str) -> Result<i32, Failure> {
    text.parse().map_err(|_| {
        // ALLOC: error path; one message.
        Failure::plain(
            "number_format_exception",
            format!("For input string: \"{text}\""),
        )
    })
}

/// `Queries.calculateMinShouldMatch`: an integer (negative counts back from
/// the clauses), a percentage (truncated), or conditional `n<spec` parts
/// separated by spaces.
pub fn calculate_min_should_match(optional: i32, spec: &str) -> Result<i32, Failure> {
    let spec = spec.trim();
    let mut result = optional;
    if spec.contains('<') {
        // `spaceAroundLessThanPattern`: whitespace around a `<` goes.
        // ALLOC: one spec a query, as long as its text.
        let mut joined = String::with_capacity(spec.len());
        for (i, part) in spec.split('<').enumerate() {
            if i > 0 {
                joined.truncate(joined.trim_end().len());
                joined.push('<');
                joined.push_str(part.trim_start());
            } else {
                joined.push_str(part);
            }
        }
        for s in joined.split(' ') {
            let mut parts = s.split('<');
            let upper = parse_int(parts.next().unwrap_or(""))?;
            if optional <= upper {
                return Ok(result);
            }
            result = calculate_min_should_match(optional, parts.next().unwrap_or(""))?;
        }
        return Ok(result);
    }
    if spec.contains('%') {
        // The `%` is taken to be last; if it is not, the number fails.
        let end = spec.char_indices().last().map_or(0, |(i, _)| i);
        let percent = parse_int(spec.get(..end).unwrap_or(""))?;
        // ARITH: Java multiplies two ints, wrapping, then scales as a float.
        #[allow(clippy::cast_precision_loss)]
        let calc = (result.wrapping_mul(percent)) as f32 * (1.0 / 100.0);
        // A float in an int's range truncates toward zero, as `(int)` does.
        #[allow(clippy::cast_possible_truncation)]
        let truncated = calc as i32;
        result = if calc < 0.0 {
            result.saturating_add(truncated)
        } else {
            truncated
        };
    } else {
        let calc = parse_int(spec)?;
        result = if calc < 0 {
            result.saturating_add(calc)
        } else {
            calc
        };
    }
    Ok(result.max(0))
}

#[cfg(test)]
#[path = "lucene_query_tests.rs"]
mod tests;

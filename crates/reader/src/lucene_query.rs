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

use crate::date::DateSpec;
use crate::dsl::{BoolQuery, Operator, Query, Value};
use crate::failure::{Failure, Meta};
use crate::mapping::{FieldType, Mapping};
use crate::numbers;

pub(crate) use crate::lucene_range::{date_range, range, range_in};

/// What a query is built against.
pub struct QueryContext<'a> {
    /// The index's fields.
    pub mapping: &'a Mapping,
    /// The analyzer of `text` fields: the standard analyzer.
    pub analyzer: &'a Analyzer,
    /// The reader's `maxDoc`, which a match-all query covers.
    pub max_doc: i32,
    /// The answers of the query's `knn` nodes, when the search ran them.
    pub knn: Option<&'a crate::knn::KnnResults>,
    /// What analyses a pinned field's text; none on a node that cannot run a
    /// pin, which then refuses a query on such a field.
    pub pinned: Option<&'a dyn crate::pinned::PinnedTokens>,
    /// The instant `now` in date math means; a date math `now` is refused
    /// without one.
    pub now_millis: Option<i64>,
    /// The shard the query runs on, whose name seeds a random score; none
    /// where a query is only checked.
    pub shard: Option<crate::shard_ref::ShardRef<'a>>,
}

/// The query `text` makes on the text field `field`: with the pin the field
/// names when it has one, never with the standard analyzer. `quoted` and
/// `slop` make a phrase; `occur` joins the words of anything else.
///
/// # Errors
///
/// `illegal_argument_exception` for the analyzer's refusal, and
/// `extension_unavailable` when the pin cannot run here or fails: the
/// query is refused rather than answered from the standard analyzer.
pub(crate) fn text_query(
    cx: &QueryContext<'_>,
    field: &str,
    text: &str,
    occur: Occur,
    quoted: bool,
    slop: u32,
) -> Result<Option<Clause>, Failure> {
    let builder = QueryBuilder::new(cx.analyzer);
    let illegal = |e: lucene_search::Error| {
        // ALLOC: error path; one message.
        Failure::plain("illegal_argument_exception", e.to_string())
    };
    let Some(pin) = cx.mapping.get(field).and_then(|f| f.analyzer.as_deref()) else {
        return builder
            .create_field_query_from_text(occur, field, text, quoted, slop)
            .map_err(illegal);
    };
    let Some(pinned) = cx.pinned else {
        return Err(Failure::plain(
            "extension_unavailable",
            // ALLOC: error path; one message.
            format!("field [{field}] is analysed by [{pin}], which this node cannot run"),
        ));
    };
    let tokens = pinned
        .tokens(pin, text)
        .map_err(|why| Failure::plain("extension_unavailable", why))?;
    let mut stream = crate::pinned::ListStream::new(tokens);
    builder
        .create_field_query(&mut stream, occur, field, quoted, slop)
        .map_err(illegal)
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
            date,
        } => (
            range_in(
                cx,
                field,
                from.as_ref(),
                to.as_ref(),
                (*include_lower, *include_upper),
                *date,
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
        FieldType::Date => date_range(
            cx,
            field,
            Some(value),
            Some(value),
            (true, true),
            DateSpec::default(),
        )?,
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
    let built = text_query(cx, field, value.text(), occur, false, 0)?;
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
pub(crate) fn apply_min_should_match(
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

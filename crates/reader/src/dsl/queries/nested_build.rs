//! `nested` built as `NestedQueryBuilder.doToQuery` builds it, at OpenSearch
//! 3.10.0: a `ToParentBlockJoinQuery` over the children of the path, whose
//! parents are the root documents, or the documents of the enclosing nested
//! object.

use std::sync::Arc;

use lucene_search::join::{QueryBitSetProducer, ScoreMode as JoinMode, ToParentBlockJoinQuery};
use lucene_search::query::FieldExistsQuery;
use lucene_search::{BooleanQuery, Clause, TermQuery};

use crate::dsl::queries::nested::{NestedQuery, ScoreMode};
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, build, match_none};

/// The field that holds a nested document's path (`NestedPathFieldMapper`).
pub const NESTED_PATH: &str = "_nested_path";

/// `Queries.newNonNestedFilter`: the documents that are not nested, which
/// are the ones that have a primary term.
pub fn non_nested() -> BooleanQuery {
    BooleanQuery {
        // ALLOC: one clause.
        filter: vec![Clause::Exists(FieldExistsQuery::new("_primary_term"))],
        ..BooleanQuery::default()
    }
}

/// `ObjectMapper.nestedTypeFilter`: the documents of the nested object `path`.
pub fn type_filter(path: &str) -> Clause {
    Clause::Term(TermQuery::new(NESTED_PATH, path.as_bytes()))
}

fn illegal_state(reason: String) -> Failure {
    Failure::plain("illegal_state_exception", reason)
}

/// Whether `path` names an object (`getObjectMapper`): some field lies under
/// it. A leaf field is no object, so it is unmapped here.
pub fn is_object(cx: &QueryContext<'_>, path: &str) -> bool {
    cx.mapping
        .names()
        .any(|n| n.strip_prefix(path).is_some_and(|r| r.starts_with('.')))
}

/// The query for `q`.
pub fn nested(q: &NestedQuery, cx: &QueryContext<'_>) -> Result<Clause, Failure> {
    if !cx.mapping.is_nested(&q.path) {
        // A mapped object that is not nested fails even when unmapped paths
        // are ignored.
        if is_object(cx, &q.path) {
            // ALLOC: error path; one message.
            return Err(illegal_state(format!(
                "[nested] nested object under path [{}] is not of nested type",
                q.path
            )));
        }
        return if q.ignore_unmapped {
            Ok(match_none())
        } else {
            // ALLOC: error path; one message.
            Err(illegal_state(format!(
                "[nested] failed to find nested object under path [{}]",
                q.path
            )))
        };
    }
    let parents = match cx.mapping.nested_parent(&q.path) {
        None => non_nested(),
        Some(parent) => BooleanQuery {
            // ALLOC: one clause.
            filter: vec![type_filter(parent)],
            ..BooleanQuery::default()
        },
    };
    let child = build(&q.query, cx)?;
    // The inner query may match documents outside the children: keep to them.
    let child = BooleanQuery {
        // ALLOC: two clauses.
        must: vec![child],
        filter: vec![type_filter(&q.path)],
        ..BooleanQuery::default()
    };
    let mode = match q.score_mode {
        ScoreMode::None => JoinMode::None,
        ScoreMode::Avg => JoinMode::Avg,
        ScoreMode::Max => JoinMode::Max,
        ScoreMode::Sum => JoinMode::Total,
        ScoreMode::Min => JoinMode::Min,
    };
    Ok(ToParentBlockJoinQuery::new(
        // ALLOC: one box a nested query.
        Clause::Boolean(Box::new(child)),
        // ALLOC: one producer a nested query.
        Arc::new(QueryBitSetProducer::new(parents)),
        mode,
    )
    .into())
}

/// `DefaultSearchContext.buildFilteredQuery`: on an index with nested
/// objects a search runs over the documents that are not nested. A `knn`
/// query there would rank children, so it is refused.
///
/// # Errors
///
/// A `knn` query on an index with nested objects.
pub fn skip_nested(
    query: BooleanQuery,
    mapping: &crate::mapping::Mapping,
    has_knn: bool,
) -> Result<BooleanQuery, Failure> {
    if !mapping.has_nested() {
        return Ok(query);
    }
    if has_knn {
        return Err(Failure::plain(
            "illegal_argument_exception",
            "a [knn] query on an index with nested objects is not supported by this node yet",
        ));
    }
    Ok(BooleanQuery {
        // ALLOC: one clause.
        must: vec![Clause::Boolean(Box::new(query))],
        // ALLOC: one clause.
        filter: vec![Clause::Boolean(Box::new(non_nested()))],
        ..BooleanQuery::default()
    })
}

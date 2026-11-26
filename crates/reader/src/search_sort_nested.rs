//! A sort that orders parents by the values of their nested children:
//! `SortBuilder.resolveNested` and `FieldSortBuilder.build` at OpenSearch
//! 3.10.0, over the engine's `ToParentBlockJoinSortField`.

use std::sync::Arc;

use lucene_search::join::{
    JoinMissing, JoinSortType, QueryBitSetProducer, ToParentBlockJoinSortField,
};
use lucene_search::top_field::{SortField, SortType};
use lucene_search::{BooleanQuery, Clause};
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::dsl::SortKey;
use crate::dsl::queries::nested_build::{non_nested, type_filter};
use crate::dsl::sort::NestedSort;
use crate::failure::{Failure, Meta};
use crate::lucene_query::{QueryContext, build, shard_failure};
use crate::search::ShardError;
use crate::search_sort::{KeyKind, field_sort};

/// A nested sort key is not served together with `search_after`: the
/// after-key of a block-join comparator is not read back.
///
/// # Errors
///
/// `illegal_argument_exception` when `source` has a `search_after` and a nested sort.
pub(crate) fn refuse_after(source: &crate::dsl::SearchSource) -> Result<(), ShardError> {
    if !source.search_after.is_empty()
        && source.sort.iter().any(|k| {
            matches!(
                k,
                SortKey::Field {
                    nested: Some(_),
                    ..
                }
            )
        })
    {
        return Err(ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            "[search_after] with a nested sort is not supported by this node yet",
        )));
    }
    Ok(())
}

/// A `_script` sort key is served on the search's own hits only: not with
/// `search_after`, in inner hits or in `top_hits`.
///
/// # Errors
///
/// `illegal_argument_exception` naming where the key sits.
pub(crate) fn refuse_script(sort: &[SortKey], where_: &str) -> Result<(), ShardError> {
    if sort.iter().any(|k| matches!(k, SortKey::Script { .. })) {
        return Err(ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!("a [_script] sort {where_} is not supported by this node yet"),
        )));
    }
    Ok(())
}

/// The sort key for the field `name` under the nested object `nested`.
pub(crate) fn nested_field_sort<S: BlobStore + 'static>(
    name: &str,
    reverse: bool,
    nested: &NestedSort,
    shard: &OpenShard<S>,
    index: &str,
    cx: Option<&QueryContext<'_>>,
) -> Result<(SortField, KeyKind), ShardError> {
    let mapping = &shard.meta.mapping;
    if !mapping.is_nested(&nested.path) {
        return Err(ShardError::bad_request(
            Failure::opensearch(
                "query_shard_exception",
                // ALLOC: error path; one message.
                format!(
                    "[nested] failed to find nested object under path [{}]",
                    nested.path
                ),
            )
            // ALLOC: error path; the index name.
            .with_meta("index", Meta::Str(index.to_owned()))
            // ALLOC: error path; the index uuid.
            .with_meta("index_uuid", Meta::Str(shard.meta.uuid.clone())),
        ));
    }
    if name == "_doc" || name == "_seq_no" {
        return Err(ShardError::bad_request(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!("sorting on [{name}] under a nested object is not supported by this node yet"),
        )));
    }
    // The field's own checks and its missing value, as without a nested object.
    let (plain, kind) = field_sort(name, reverse, shard, index)?;
    let (ty, parent_missing) = match (kind, plain.ty) {
        (KeyKind::Term, _) => (
            JoinSortType::String,
            // Missing values last in either direction; a reversed order puts them first
            // in the raw one.
            if reverse {
                JoinMissing::StringFirst
            } else {
                JoinMissing::StringLast
            },
        ),
        (_, SortType::Int) => (
            JoinSortType::Int,
            JoinMissing::Int(i32::try_from(plain.missing).unwrap_or(i32::MAX)),
        ),
        _ => (JoinSortType::Long, JoinMissing::Long(plain.missing)),
    };
    let parents = match mapping.nested_parent(&nested.path) {
        None => non_nested(),
        Some(parent) => BooleanQuery {
            // ALLOC: one clause.
            filter: vec![type_filter(parent)],
            ..BooleanQuery::default()
        },
    };
    // The children: the filter's matches as the Java's one-level sort has it,
    // else the documents of the path.
    let children = match &nested.filter {
        None => BooleanQuery {
            // ALLOC: one clause.
            filter: vec![type_filter(&nested.path)],
            ..BooleanQuery::default()
        },
        Some(filter) => {
            let Some(cx) = cx else {
                return Err(ShardError::bad_request(Failure::plain(
                    "illegal_argument_exception",
                    "a nested sort filter in a [top_hits] is not supported by this node yet",
                )));
            };
            let clause = build(filter, cx)
                .map_err(|f| ShardError::of_query(shard_failure(f, index, &shard.meta.uuid)))?;
            BooleanQuery {
                // ALLOC: one clause.
                filter: vec![unscored(clause)],
                ..BooleanQuery::default()
            }
        }
    };
    let sort = ToParentBlockJoinSortField::new(
        name,
        ty,
        reverse,
        // The greatest child when descending, the least when ascending.
        reverse,
        Some(parent_missing),
        // A string child without a value is past the terms when taking the least
        // child, and before them when taking the greatest, so it is never the pick.
        (ty == JoinSortType::String && !reverse).then_some(JoinMissing::StringLast),
        // ALLOC: one producer a sort key.
        Arc::new(QueryBitSetProducer::new(parents)),
        // ALLOC: one producer a sort key.
        Arc::new(QueryBitSetProducer::new(children)),
    )
    .map_err(ShardError::engine)?;
    Ok((sort.sort_field(), kind))
}

/// `clause` without the constant-score and boost wrappers a keyword term
/// carries: the engine's bit set of a block-join sort does not run them, and
/// a set of children has no use for a score.
fn unscored(clause: Clause) -> Clause {
    match clause {
        Clause::ConstantScore(c) => unscored(*c.inner),
        Clause::Boost(b) => unscored(*b.inner),
        Clause::Boolean(b) => {
            // ALLOC: rewrites the filter's own clauses; bounded by the body limit.
            let b = *b;
            Clause::Boolean(Box::new(BooleanQuery {
                // ALLOC: one clause per clause of the filter.
                must: b.must.into_iter().map(unscored).collect(),
                // ALLOC: one clause per clause of the filter.
                filter: b.filter.into_iter().map(unscored).collect(),
                // ALLOC: one clause per clause of the filter.
                should: b.should.into_iter().map(unscored).collect(),
                // ALLOC: one clause per clause of the filter.
                must_not: b.must_not.into_iter().map(unscored).collect(),
                ..b
            }))
        }
        other => other,
    }
}

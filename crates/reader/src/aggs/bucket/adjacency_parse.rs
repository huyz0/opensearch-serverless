//! `adjacency_matrix`'s body: `AdjacencyMatrixAggregationBuilder.PARSER` at
//! OpenSearch 3.10.0, composed into the filters the engine counts: each
//! filter and, for every pair of filters, the query matching both, under the
//! key `a<separator>b`, all sorted by key as the reduce sorts the buckets.

use crate::aggs::bucket::filters::{FiltersSpec, compare_keys};
use crate::dsl::BoolQuery;
use crate::dsl::Query;
use crate::dsl::error::DslError;
use crate::dsl::query::parse_inner;
use crate::dsl::tokens::{Kind, Parser};

/// The most filters one `adjacency_matrix` holds (`index.max_adjacency_matrix_filters`).
const MAX_FILTERS: usize = 100;

fn typed(p: &Parser<'_>, name: &str, kind: Kind) -> DslError {
    DslError::xcontent(
        p.location(),
        // ALLOC: error path; one message.
        &format!(
            "[adjacency_matrix] {name} doesn't support values of type: {}",
            kind.name()
        ),
        None,
    )
}

/// One filter of the `filters` object, read as `parseInnerQueryBuilder`
/// reads it, a failure wrapped as the object parser wraps it.
fn filter(
    p: &mut Parser<'_>,
    key: &str,
    outer: oss_rest::json::Location,
) -> Result<Query, DslError> {
    parse_inner(p).map_err(|cause| {
        let inner = DslError::xcontent(
            p.location(),
            // ALLOC: error path; one message.
            &format!("[filters] failed to parse field [{key}]"),
            Some(cause),
        );
        DslError::xcontent(
            outer,
            "[adjacency_matrix] failed to parse field [filters]",
            Some(inner),
        )
    })
}

/// The options of an `adjacency_matrix` body, the parser on its start.
pub fn parse_adjacency_body(p: &mut Parser<'_>, name: &str) -> Result<FiltersSpec, DslError> {
    // ALLOC: one separator, bounded by the body's recording.
    let mut separator = "&".to_owned();
    let mut only = false;
    let mut filters: Option<Vec<(String, Query)>> = None;
    let mut key = "";
    loop {
        let kind = match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                key = p.text();
                if !matches!(key, "filters" | "separator" | "show_only_intersecting") {
                    return Err(DslError::xcontent(
                        p.location(),
                        // ALLOC: error path; one message.
                        &format!("[adjacency_matrix] unknown field [{key}]"),
                        None,
                    ));
                }
                continue;
            }
            Some(kind) => kind,
        };
        match (kind, key) {
            (Kind::StartObject, "filters") => {
                let outer = p.location();
                // ALLOC: one filter, bounded by the body's recording.
                let mut list: Vec<(String, Query)> = Vec::new();
                // ALLOC: empty; a key a filter replaces it.
                let mut filter_key = String::new();
                loop {
                    match p.next_token() {
                        Some(Kind::EndObject) | None => break,
                        // ALLOC: one key a filter, bounded by the body's recording.
                        Some(Kind::FieldName) => filter_key = p.text().to_owned(),
                        Some(_) => {
                            let query = filter(p, &filter_key, outer)?;
                            // ALLOC: one filter, bounded by the body's recording.
                            list.push((filter_key.clone(), query));
                        }
                    }
                }
                filters = Some(list);
            }
            (Kind::StartArray, "filters") => {
                return Err(DslError::xcontent(
                    p.location(),
                    "[adjacency_matrix] failed to parse field [filters]",
                    Some(DslError::illegal(
                        "[filters] doesn't support arrays. Use a single object with multiple fields.",
                    )),
                ));
            }
            // ALLOC: one separator, bounded by the body's recording.
            (Kind::String, "separator") => separator = p.text().to_owned(),
            (Kind::Bool, "show_only_intersecting") => only = p.text() == "true",
            (Kind::String, "show_only_intersecting") => match p.text() {
                "true" => only = true,
                "false" => only = false,
                other => {
                    return Err(DslError::xcontent(
                        p.location(),
                        "[adjacency_matrix] failed to parse field [show_only_intersecting]",
                        Some(DslError::illegal(
                            // ALLOC: error path; one message.
                            format!(
                                "Failed to parse value [{other}] as only [true] or [false] are allowed."
                            ),
                        )),
                    ));
                }
            },
            _ => return Err(typed(p, key, kind)),
        }
    }
    let mut filters = match filters {
        Some(list) if !list.is_empty() => list,
        _ => {
            return Err(DslError::Failure(crate::failure::Failure::plain(
                "illegal_state_exception",
                // ALLOC: error path; one message.
                format!("[{name}] is missing : filters parameter"),
            )));
        }
    };
    if filters.len() > MAX_FILTERS {
        return Err(DslError::unsupported(
            "[adjacency_matrix] with more than 100 filters",
        ));
    }
    filters.sort_by(|a, b| compare_keys(&a.0, &b.0));
    // ALLOC: one filter and pair of filters, bounded by 100 filters.
    let mut all: Vec<(String, Query)> = Vec::new();
    for (i, (a, qa)) in filters.iter().enumerate() {
        for (b, qb) in filters.iter().skip(i.saturating_add(1)) {
            let both = BoolQuery {
                // ALLOC: two filters a pair, bounded by 100 filters.
                must: vec![qa.clone(), qb.clone()],
                adjust_pure_negative: true,
                boost: 1.0,
                ..BoolQuery::default()
            };
            // ALLOC: one key and box a pair, bounded by 100 filters.
            all.push((format!("{a}{separator}{b}"), Query::Bool(Box::new(both))));
        }
    }
    if !only {
        all.extend(filters);
    }
    all.sort_by(|a, b| compare_keys(&a.0, &b.0));
    Ok(FiltersSpec {
        filters: all,
        ..FiltersSpec::default()
    })
}

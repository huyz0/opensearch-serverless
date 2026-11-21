//! `filters`' body: `FiltersAggregationBuilder.parse` at OpenSearch 3.10.0,
//! each filter read as `AbstractQueryBuilder.parseInnerQueryBuilder` reads a
//! query, and the builder's sort of keyed filters.

use crate::aggs::bucket::filters::{DEFAULT_OTHER_KEY, FiltersSpec, compare_keys};
use crate::dsl::error::DslError;
use crate::dsl::query::parse_inner;
use crate::dsl::tokens::{Kind, Parser};

/// The options of a `filter` body, the parser on its start: the one filter,
/// read as `FilterAggregationBuilder.parse` reads it.
pub fn parse_filter_body(p: &mut Parser<'_>) -> Result<FiltersSpec, DslError> {
    let query = parse_inner(p)?;
    Ok(FiltersSpec {
        single: true,
        // ALLOC: one filter, bounded by the body's recording.
        filters: vec![("0".to_owned(), query)],
        ..FiltersSpec::default()
    })
}

/// The options of a `filters` body, the parser on its start.
pub fn parse_filters_body(p: &mut Parser<'_>, name: &str) -> Result<FiltersSpec, DslError> {
    let mut spec = FiltersSpec::default();
    let mut other: Option<bool> = None;
    let mut other_key: Option<String> = None;
    let mut key = "";
    loop {
        let kind = match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                key = p.text();
                continue;
            }
            Some(kind) => kind,
        };
        match (kind, key) {
            (Kind::Bool, "other_bucket") => other = Some(p.text() == "true"),
            // ALLOC: one key, bounded by the body's recording.
            (Kind::String, "other_bucket_key") => other_key = Some(p.text().to_owned()),
            (Kind::StartObject, "filters") => {
                spec.keyed = true;
                // ALLOC: empty; a key a filter replaces it.
                let mut filter_key = String::new();
                loop {
                    match p.next_token() {
                        Some(Kind::EndObject) | None => break,
                        // ALLOC: one key a filter, bounded by the body's recording.
                        Some(Kind::FieldName) => filter_key = p.text().to_owned(),
                        Some(_) => {
                            let query = parse_inner(p)?;
                            // ALLOC: one filter, bounded by the body's recording.
                            spec.filters.push((filter_key.clone(), query));
                        }
                    }
                }
            }
            (Kind::StartArray, "filters") => {
                while p.next_token() != Some(Kind::EndArray) {
                    if p.current_token().is_none() {
                        break;
                    }
                    let query = parse_inner(p)?;
                    // ALLOC: one filter, bounded by the body's recording.
                    spec.filters.push((spec.filters.len().to_string(), query));
                }
            }
            _ => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!("Unknown key for a {} in [{name}]: [{key}].", kind.name()),
                ));
            }
        }
    }
    if spec.filters.is_empty() {
        return Err(DslError::illegal("[filters] cannot be empty."));
    }
    if spec.keyed {
        // The builder keeps keyed filters in a fixed order, whatever the request's.
        spec.filters.sort_by(|a, b| compare_keys(&a.0, &b.0));
    }
    // A key asked for turns the other bucket on, unless it is turned off.
    let other = other.unwrap_or(other_key.is_some());
    if other {
        // ALLOC: one key, bounded by the body's recording.
        spec.other_key = Some(other_key.unwrap_or_else(|| DEFAULT_OTHER_KEY.to_owned()));
    }
    Ok(spec)
}

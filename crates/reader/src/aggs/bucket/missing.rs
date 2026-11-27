//! `missing`'s body: `MissingAggregationBuilder`'s `ObjectParser`
//! (`ValuesSourceAggregationBuilder.declareFields`) at OpenSearch 3.10.0, and
//! the filter that counts what `MissingAggregator` counts: the documents with
//! no value for the field.

use crate::aggs::bucket::filters::FiltersSpec;
use crate::dsl::Query;
use crate::dsl::bool_query::BoolQuery;
use crate::dsl::error::DslError;
use crate::dsl::queries::More;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::suggest;

/// Every key `missing` declares.
const KEYS: &[&str] = &["field", "missing", "value_type", "format", "script"];

/// The field and the one filter of a `missing` body, the parser on its
/// start: the filter is `bool { must_not: [exists { field }] }`, so a field
/// the mapping does not know counts every match.
pub fn parse_missing_body(p: &mut Parser<'_>) -> Result<(String, FiltersSpec), DslError> {
    let mut field: Option<String> = None;
    let mut key = "";
    let mut key_at = p.location();
    loop {
        let kind = match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                key = p.text();
                key_at = p.location();
                continue;
            }
            Some(kind) => kind,
        };
        if !KEYS.contains(&key) {
            return Err(DslError::xcontent(
                key_at,
                // ALLOC: error path; one message.
                &format!("[missing] unknown field [{key}]{}", suggest(key, KEYS)),
                None,
            ));
        }
        if key != "field" {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!(
                "the [missing] aggregation's [{key}]"
            )));
        }
        if kind != Kind::String {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[missing] field doesn't support values of type: {}",
                    kind.name()
                ),
                None,
            ));
        }
        // ALLOC: one field name an aggregation, bounded by the body's recording.
        field = Some(p.text().to_owned());
    }
    let field = field.ok_or_else(|| {
        DslError::illegal("Required one of fields [field, script], but none were specified. ")
    })?;
    // ALLOC: one exists query, bounded by the body's recording.
    let exists = Query::More(Box::new(More::Exists {
        // ALLOC: one field name, bounded by the body's recording.
        field: field.clone(),
        boost: 1.0,
    }));
    // ALLOC: one bool query, bounded by the body's recording.
    let query = Query::Bool(Box::new(BoolQuery {
        // ALLOC: one clause, bounded by the body's recording.
        must_not: vec![exists],
        adjust_pure_negative: true,
        boost: 1.0,
        ..BoolQuery::default()
    }));
    let spec = FiltersSpec {
        single: true,
        // ALLOC: one filter, bounded by the body's recording.
        filters: vec![("0".to_owned(), query)],
        ..FiltersSpec::default()
    };
    Ok((field, spec))
}

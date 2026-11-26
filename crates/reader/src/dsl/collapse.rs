//! `collapse`: `CollapseBuilder`'s object parser at OpenSearch 3.10.0.

use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::int_value;

/// A parsed `collapse`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collapse {
    /// The keyword or number field whose values group the hits.
    pub field: String,
}

/// The `collapse` object, the parser on its start.
///
/// # Errors
///
/// A [`DslError`] for what stock refuses, and for `inner_hits`, which this
/// node does not serve.
pub fn parse(p: &mut Parser<'_>) -> Result<Collapse, DslError> {
    let mut field = None;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(k) if k.is_value() && current == "field" => {
                // ALLOC: one name a collapse.
                field = Some(p.text().to_owned());
            }
            Some(k) if k.is_value() && current == "max_concurrent_group_searches" => {
                if int_value(p)? < 1 {
                    return Err(DslError::illegal(
                        "maxConcurrentGroupRequests` must be positive",
                    ));
                }
            }
            Some(Kind::StartObject | Kind::StartArray) if current == "inner_hits" => {
                return Err(DslError::unsupported("[collapse] [inner_hits]"));
            }
            Some(_) => {
                return Err(DslError::xcontent(
                    p.location(),
                    // ALLOC: error path; one message.
                    &format!("[collapse] unknown field [{current}]"),
                    None,
                ));
            }
        }
    }
    match field {
        None => Err(DslError::illegal("Required [field]")),
        Some(f) if f.is_empty() => Err(DslError::illegal("field name is null or empty")),
        Some(field) => Ok(Collapse { field }),
    }
}

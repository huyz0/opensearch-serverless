//! The shape most term-level parsers share: `{"<field>": <value>}` or
//! `{"<field>": {"<param>": <value>, ...}}`, one field only.
//!
//! Ported from the loop `PrefixQueryBuilder`, `WildcardQueryBuilder`,
//! `RegexpQueryBuilder` and `FuzzyQueryBuilder` each spell out in
//! `fromXContent`, with `AbstractQueryBuilder.throwParsingExceptionOnMultipleFields`,
//! at OpenSearch 3.10.0.

use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};

/// `throwParsingExceptionOnMultipleFields`.
pub fn multiple_fields(p: &Parser<'_>, query: &str, first: &str, second: &str) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!("[{query}] query doesn't support multiple fields, found [{first}] and [{second}]"),
    )
}

/// `"[<query>] query does not support [<param>]"`.
pub fn does_not_support(p: &Parser<'_>, query: &str, param: &str) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!("[{query}] query does not support [{param}]"),
    )
}

/// Reads one field's clause of query `query`: the parser is on the query
/// object's start and ends on its end. `on_value` is called on each value
/// token, with `Some(param)` in the long form and `None` for the short
/// form's value. Returns the field's name, `None` when the object is empty.
pub fn parse<'r>(
    p: &mut Parser<'r>,
    query: &str,
    mut on_value: impl FnMut(&Parser<'r>, Option<&'r str>) -> Result<(), DslError>,
) -> Result<Option<&'r str>, DslError> {
    let mut field: Option<&'r str> = None;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => return Ok(field),
            Some(Kind::FieldName) => current = p.text(),
            Some(Kind::StartObject) => {
                if let Some(f) = field {
                    return Err(multiple_fields(p, query, f, current));
                }
                field = Some(current);
                loop {
                    match p.next_token() {
                        Some(Kind::EndObject) | None => break,
                        Some(Kind::FieldName) => current = p.text(),
                        Some(k) if k.is_value() => on_value(p, Some(current))?,
                        Some(k) => {
                            return Err(DslError::parsing(
                                p.location(),
                                // ALLOC: error path; one message.
                                format!(
                                    "[{query}] unexpected token [{}] after [{current}]",
                                    k.name()
                                ),
                            ));
                        }
                    }
                }
            }
            Some(k) => {
                let name = p.current_name().unwrap_or("");
                if let Some(f) = field {
                    return Err(multiple_fields(p, query, f, name));
                }
                if !k.is_value() {
                    return Err(DslError::parsing(
                        p.location(),
                        // ALLOC: error path; one message.
                        format!("[{query}] unexpected token [{}] after [{name}]", k.name()),
                    ));
                }
                field = Some(current);
                on_value(p, None)?;
            }
        }
    }
}

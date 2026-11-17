//! A document read against a mapping, as OpenSearch's `DocumentParser`
//! reads one: the values each mapped field indexes, the fields left in
//! `_ignored`, and the mapping update dynamic mapping makes, with the
//! Java's refusals and messages.
//!
//! The document is pulled token by token from its bytes; nothing but the
//! values and the update is built from it.

mod dynamic;
mod preview;
mod reader;
mod values;
mod walk;

pub use values::{FieldValue, parse_date};

use crate::mapping::{Mapping, MappingError, Object};

/// What a document gives its index.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedDocument {
    /// Each value a field indexes, by the field's full name, in document
    /// order; a multi-field follows its parent's value.
    pub fields: Vec<(String, FieldValue)>,
    /// Fields that kept a value out of the index (`ignore_above`,
    /// `ignore_malformed`), for `_ignored`, in document order.
    pub ignored: Vec<String>,
    /// The fields and objects dynamic mapping added, as a mapping of their
    /// own, or `None` when it added none.
    pub update: Option<Mapping>,
    /// The index's mapping with `update` merged in, when there is one.
    pub grown: Option<Mapping>,
}

/// Reads `source`, the document with id `id`, against `mapping`.
///
/// # Errors
///
/// The [`MappingError`] OpenSearch answers the index request with: a
/// value a field cannot read, a field `dynamic: strict` refuses, a bad
/// field name, malformed JSON (`mapper_parsing_exception` with the cause),
/// or a dynamic field whose values disagree on its type
/// (`illegal_argument_exception`).
pub fn parse(mapping: &Mapping, id: &str, source: &[u8]) -> Result<ParsedDocument, MappingError> {
    let mut parser = walk::Parser::new(mapping, id, source);
    parser.document().map_err(wrap)?;
    let (mut doc, update, conflict) = parser.finish();
    if let Some(e) = conflict {
        return Err(e);
    }
    if !update.children().is_empty() {
        let mut delta = Mapping::default();
        *delta.root_mut() = update;
        doc.grown = Some(mapping.merge(&delta)?);
        doc.update = Some(delta);
    }
    Ok(doc)
}

/// `wrapInMapperParsingException`: a mapper parsing error as it is, any
/// other as the cause of `failed to parse`.
fn wrap(e: MappingError) -> MappingError {
    if matches!(
        e.ty,
        "mapper_parsing_exception" | "strict_dynamic_mapping_exception"
    ) {
        e
    } else {
        MappingError::mapper_parsing("failed to parse").caused_by(e)
    }
}

/// Splits a field name on dots as `splitAndValidatePath` does, with
/// `String.split`'s dropping of trailing empty parts.
fn split_path(name: &str) -> Result<Vec<&str>, MappingError> {
    if !name.contains('.') {
        if name.is_empty() {
            return Err(MappingError::illegal_argument(
                "field name cannot be an empty string",
            ));
        }
        // ALLOC: one part; parse path.
        return Ok(vec![name]);
    }
    // ALLOC: one entry a part, bounded by the name's length.
    let mut parts: Vec<&str> = name.split('.').collect();
    while parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    if parts.is_empty() {
        return Err(MappingError::illegal_argument(
            "field name cannot contain only the character [.]",
        ));
    }
    for part in &parts {
        if part.chars().all(char::is_whitespace) {
            return Err(if part.is_empty() {
                // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                MappingError::illegal_argument(&format!(
                    "object field starting or ending with a [.] makes object resolution ambiguous: [{name}]"
                ))
            } else {
                // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                MappingError::illegal_argument(&format!(
                    "object field cannot contain only whitespace: ['{name}']"
                ))
            });
        }
    }
    Ok(parts)
}

/// An empty object, as dynamic mapping adds one.
fn dynamic_object() -> Object {
    Object::new()
}

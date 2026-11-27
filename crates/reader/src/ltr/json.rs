//! Reading a definition's JSON through the search body's token recorder.

use oss_rest::json::{Limits, Tokenizer};

use super::ModelError;
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser, Recorder};

/// Records `text`, bounded by its own length (a token and a byte of text
/// each cost at most a few bytes).
pub(crate) fn record(text: &str) -> Result<Recorder, ModelError> {
    let limit = text.len().saturating_mul(32).saturating_add(1 << 12);
    let mut rec = Recorder::new(limit);
    let mut tokenizer = Tokenizer::new(Limits::default());
    let bad = |e: DslError| match e {
        // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES.
        DslError::Json(j) => ModelError::new(format!("the definition is not JSON: {j}")),
        _ => ModelError::new("the definition is too large to read"),
    };
    tokenizer.feed(text.as_bytes(), &mut rec).map_err(bad)?;
    tokenizer.finish(&mut rec).map_err(bad)?;
    Ok(rec)
}

/// A number token as `f32`.
pub(super) fn number(p: &Parser<'_>) -> Result<f32, ModelError> {
    if p.current_token() != Some(Kind::Number) {
        return Err(at(p, "expected a number"));
    }
    p.text()
        .parse::<f32>()
        // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES.
        .map_err(|_| at(p, &format!("[{}] is not a number", p.text())))
}

/// An error that names where in the definition the parser stands.
pub(super) fn at(p: &Parser<'_>, what: &str) -> ModelError {
    let loc = p.location();
    // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES.
    ModelError::new(format!(
        "{what} at line {} column {} of the definition",
        loc.line, loc.column
    ))
}

/// Reads an array of numbers, the parser on its `START_ARRAY`; stops on `END_ARRAY`.
pub(super) fn numbers(p: &mut Parser<'_>, most: usize) -> Result<Vec<f32>, ModelError> {
    if p.current_token() != Some(Kind::StartArray) {
        return Err(at(p, "expected an array"));
    }
    // ALLOC: at most `most` entries, checked per entry.
    let mut out = Vec::new();
    loop {
        match p.next_token() {
            Some(Kind::EndArray) => return Ok(out),
            Some(Kind::Number) => {
                if out.len() >= most {
                    return Err(at(p, "the array is longer than a model may hold"));
                }
                out.push(number(p)?);
            }
            Some(Kind::Bool) => {
                if out.len() >= most {
                    return Err(at(p, "the array is longer than a model may hold"));
                }
                out.push(if p.text() == "true" { 1.0 } else { 0.0 });
            }
            _ => return Err(at(p, "expected a number in the array")),
        }
    }
}

/// Walks the fields of the object the parser stands on, handing each name
/// and the parser on the field's value to `f`, which leaves the parser on
/// the value's last token (a scalar itself, or its closing token).
pub(super) fn each_field<'r>(
    p: &mut Parser<'r>,
    mut f: impl FnMut(&str, &mut Parser<'r>) -> Result<(), ModelError>,
) -> Result<(), ModelError> {
    if p.current_token() != Some(Kind::StartObject) {
        return Err(at(p, "expected an object"));
    }
    loop {
        match p.next_token() {
            Some(Kind::EndObject) => return Ok(()),
            Some(Kind::FieldName) => {
                let name = p.text();
                p.next_token();
                f(name, p)?;
            }
            _ => return Err(at(p, "expected a field name")),
        }
    }
}

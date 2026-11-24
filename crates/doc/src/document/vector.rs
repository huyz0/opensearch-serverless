//! A `knn_vector` field's value, as `KNNVectorFieldMapper.parseCreateField`
//! reads a float vector: an array of numbers or numeric strings of the
//! mapping's dimension, `null` for nothing, and the plugin's refusals.

use crate::mapping::{Leaf, MappingError, Value, java_parse_double};

use super::preview::preview;
use super::reader::{At, Tok};
use super::values::{FieldValue, Taken, coercion};
use super::walk::Parser;

/// The dimension the mapping fixed.
fn dimension(leaf: &Leaf) -> i32 {
    match leaf.get("dimension") {
        Some(Value::Int(d)) => *d,
        _ => crate::mapping::KNN_NO_DIMENSION,
    }
}

/// Whether the field's space is cosine, which refuses a vector of zeros.
fn cosine(leaf: &Leaf) -> bool {
    matches!(leaf.get("method"), Some(Value::Method(m)) if m.space.as_deref() == Some("cosinesimil"))
}

/// What one element or value reads as, with what the plugin previews of it.
pub(super) struct Failure {
    pub(super) cause: MappingError,
    pub(super) preview: String,
}

fn fail(cause: MappingError, shown: &str) -> Failure {
    Failure {
        cause,
        // ALLOC: error path; one message.
        preview: shown.to_owned(),
    }
}

/// `Float.parseFloat` of a string: the plugin's `NumberFormatException`.
fn parse_text(text: &str) -> Result<f32, Failure> {
    match java_parse_double(text) {
        // ALLOC: error path; one message.
        None => Err(fail(MappingError::number_format(text), text)),
        Some(d) => checked(d),
    }
}

/// A component, as `float`: `NaN` and the infinities are refused.
fn checked(d: f64) -> Result<f32, Failure> {
    #[allow(clippy::cast_possible_truncation)]
    let f = d as f32;
    if f.is_nan() {
        return Err(fail(
            MappingError::illegal_argument("KNN vector values cannot be NaN"),
            "NaN",
        ));
    }
    if f.is_infinite() {
        return Err(fail(
            MappingError::illegal_argument("KNN vector values cannot be infinity"),
            if f.is_sign_positive() {
                "Infinity"
            } else {
                "-Infinity"
            },
        ));
    }
    Ok(f)
}

/// The vector, once every element is read: its dimension and, in a cosine
/// space, a norm other than zero.
fn finish(leaf: &Leaf, vector: Vec<f32>) -> Result<Taken, Failure> {
    let expected = dimension(leaf);
    if i64::try_from(vector.len()).ok() != Some(i64::from(expected)) {
        return Err(fail(
            MappingError::illegal_argument(&format!(
                "Vector dimension mismatch. Expected: {expected}, Given: {}",
                vector.len()
            )),
            "null",
        ));
    }
    if cosine(leaf) && vector.iter().all(|c| *c == 0.0) {
        return Err(fail(
            MappingError::illegal_argument(
                "zero vector is not supported when space type is [cosinesimil]",
            ),
            "null",
        ));
    }
    Ok(Taken::Value(FieldValue::Vector(vector)))
}

/// `Base64.getDecoder().decode`'s refusal of the first byte outside its
/// alphabet. A string that is valid base64 is another encoding of the
/// vector, which is not read here.
fn base64(name: &str, text: &str) -> Failure {
    const ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=";
    let illegal = text
        .chars()
        .find(|c| !ALPHABET.contains(*c) && c.is_ascii());
    let shown = text;
    match illegal {
        Some(c) => {
            let message = format!("Illegal base64 character {:x}", c as u32);
            let cause = MappingError::illegal_argument(&format!(
                "Invalid base64 encoding for vector field [{name}]: {message}"
            ))
            .caused_by(MappingError::illegal_argument(&message));
            fail(cause, shown)
        }
        None => fail(MappingError::unsupported("knn_vector base64 value"), shown),
    }
}

/// A value that is not an array: `null` is nothing, a number a vector of
/// one, an object a vector of none, a string base64.
fn take(leaf: &Leaf, name: &str, tok: &Tok) -> Result<Taken, Failure> {
    match tok {
        Tok::Null => Ok(Taken::Nothing),
        Tok::Num(t) => match java_parse_double(t) {
            None => Err(fail(MappingError::number_format(t), t)),
            Some(d) => finish(leaf, vec![checked(d)?]),
        },
        Tok::BeginObject => finish(leaf, Vec::new()),
        Tok::Str(s) => Err(base64(name, s)),
        other => Err(fail(
            MappingError::unsupported("knn_vector value"),
            other.jackson_name(),
        )),
    }
}

impl Parser<'_, '_> {
    /// A vector field's value that is not an array.
    pub(super) fn vector_value(
        &mut self,
        leaf: &Leaf,
        full: &str,
        tok: Tok,
    ) -> Result<(), MappingError> {
        let name = full.rsplit('.').next().unwrap_or(full);
        match take(leaf, name, &tok) {
            Ok(Taken::Value(v)) => {
                // ALLOC: one value a field of the document.
                self.out.fields.push((full.to_owned(), v));
                Ok(())
            }
            Ok(Taken::Nothing) => Ok(()),
            Err(mut f) => {
                if tok == Tok::BeginObject {
                    let shown = self.reader.read_value(tok)?;
                    f.preview = preview(&shown, false);
                }
                Err(self.vector_error(leaf, full, f))
            }
        }
    }

    /// A vector's array, after its `[`: each element a component.
    pub(super) fn vector_array(&mut self, leaf: &Leaf, full: &str) -> Result<(), MappingError> {
        // ALLOC: one vector, at most the dimension (16,000) floats.
        let mut vector =
            Vec::with_capacity(usize::try_from(dimension(leaf)).unwrap_or(0).min(16_000));
        let taken = loop {
            let tok = self.reader.next()?;
            let at = self.reader.at();
            let element = match &tok {
                Tok::EndArray => break finish(leaf, vector),
                Tok::Num(t) => java_parse_double(t)
                    .ok_or_else(|| fail(MappingError::number_format(t), t))
                    .and_then(checked),
                Tok::Str(s) => parse_text(s),
                Tok::Bool(_) | Tok::Null | Tok::BeginArray => {
                    let cause = element_coercion(&tok, at);
                    let shown = self.reader.read_value(tok)?;
                    break Err(fail(cause, &preview(&shown, false)));
                }
                _ => {
                    break Err(fail(
                        MappingError::unsupported("knn_vector element"),
                        tok.jackson_name(),
                    ));
                }
            };
            match element {
                Ok(f) => vector.push(f),
                Err(e) => break Err(e),
            }
        };
        match taken {
            Ok(Taken::Value(v)) => {
                // ALLOC: one value a field of the document.
                self.out.fields.push((full.to_owned(), v));
                Ok(())
            }
            Ok(Taken::Nothing) => Ok(()),
            Err(f) => Err(self.vector_error(leaf, full, f)),
        }
    }

    /// The refusal of a value, as `FieldMapper.parse` wraps it.
    pub(super) fn vector_error(&self, leaf: &Leaf, full: &str, f: Failure) -> MappingError {
        // ALLOC: error path; one message.
        MappingError::mapper_parsing(&format!(
            "failed to parse field [{full}] of type [{}] in document with id '{}'. Preview of field's value: '{}'",
            leaf.ty().name(),
            self.id,
            f.preview
        ))
        .caused_by(f.cause)
    }
}

/// Jackson's refusal of a number accessor on `tok`.
fn element_coercion(tok: &Tok, at: At) -> MappingError {
    coercion(
        &format!(
            "Current token ({}) not numeric, cannot use numeric value accessors",
            tok.jackson_name()
        ),
        at,
    )
}

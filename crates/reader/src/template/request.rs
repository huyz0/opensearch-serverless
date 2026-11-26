//! The body of `_search/template` and `_render/template`: `SearchTemplateRequest`'s
//! `ObjectParser`, and the params as `XContentParser.map()` reads them.

use super::Val;
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser, Recorder};

/// The deepest nesting of arrays and objects a params value may have.
const MAX_DEPTH: usize = 256;

/// What a template request asks for.
#[derive(Debug, Default, PartialEq)]
pub struct TemplateRequest {
    /// The inline template: `source`, `inline` or `template`; an object is
    /// written as compact JSON.
    pub source: Option<String>,
    /// The id of a stored template.
    pub id: Option<String>,
    /// Whether the body's last word on the template was `id`, not a source:
    /// the later of the two wins, as in stock's parser.
    pub stored: bool,
    /// The `params`.
    pub params: Option<Val>,
    /// `explain`.
    pub explain: bool,
    /// `profile`.
    pub profile: bool,
    /// A non-empty `search_pipeline`.
    pub search_pipeline: bool,
}

fn unsupported_type(p: &Parser<'_>, field: &str, kind: Option<Kind>) -> DslError {
    DslError::xcontent(
        p.location(),
        // ALLOC: the template, its params and its output, which the request limits bound.
        &format!(
            "[search_template] {field} doesn't support values of type: {}",
            kind.map_or("null", Kind::name)
        ),
        None,
    )
}

/// A number as `parser.map()` reads it: an integer is an `Integer`, `Long`
/// or `BigInteger`, anything with a fraction or exponent a `Double`.
fn number(text: &str) -> Val {
    if text.contains(['.', 'e', 'E']) {
        return Val::Double(text.parse().unwrap_or(f64::NAN));
    }
    match text.parse::<i64>() {
        Ok(i) => Val::Int(i),
        // ALLOC: one number of the body, which the recording bounds.
        Err(_) => Val::Big(text.to_owned()),
    }
}

/// The value the parser stands on, read to its end.
pub fn read_val(p: &mut Parser<'_>, depth: usize) -> Result<Val, DslError> {
    if depth > MAX_DEPTH {
        return Err(DslError::unsupported("params nested this deep"));
    }
    Ok(match p.current_token() {
        Some(Kind::StartObject) => {
            // ALLOC: one entry a key of the body, which the recording bounds.
            let mut entries = Vec::new();
            while let Some(kind) = p.next_token() {
                match kind {
                    Kind::EndObject => break,
                    Kind::FieldName => {
                        // ALLOC: one key of the body, which the recording bounds.
                        let key = p.text().to_owned();
                        p.next_token();
                        let value = read_val(p, depth.saturating_add(1))?;
                        Val::put(&mut entries, key, value);
                    }
                    _ => return Err(DslError::illegal("malformed object")),
                }
            }
            Val::Map(entries)
        }
        Some(Kind::StartArray) => {
            // ALLOC: one entry an element of the body, which the recording bounds.
            let mut items = Vec::new();
            while let Some(kind) = p.next_token() {
                if kind == Kind::EndArray {
                    break;
                }
                items.push(read_val(p, depth.saturating_add(1))?);
            }
            Val::List(items)
        }
        // ALLOC: one string of the body, which the recording bounds.
        Some(Kind::String) => Val::Str(p.text().to_owned()),
        Some(Kind::Number) => number(p.text()),
        Some(Kind::Bool) => Val::Bool(p.text() == "true"),
        Some(Kind::Null) => Val::Null,
        _ => return Err(DslError::illegal("malformed value")),
    })
}

impl TemplateRequest {
    /// Parses a recorded body.
    ///
    /// # Errors
    ///
    /// [`DslError`] for a body `SearchTemplateRequest.fromXContent` refuses.
    pub fn parse(rec: &Recorder) -> Result<Self, DslError> {
        if rec.is_empty() {
            return Err(DslError::illegal(
                "request body or source parameter is required",
            ));
        }
        let mut request = Self::default();
        let mut p = rec.parser();
        let token = p.next_token();
        if token != Some(Kind::StartObject) {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[search_template] Expected START_OBJECT but was: {}",
                    token.map_or("null", Kind::name)
                ),
                None,
            ));
        }
        while let Some(kind) = p.next_token() {
            if kind == Kind::EndObject {
                break;
            }
            // ALLOC: one name of the body, which the recording bounds.
            let name = p.text().to_owned();
            let name_at = p.location();
            let value = p.next_token();
            match name.as_str() {
                "params" => {
                    if value != Some(Kind::StartObject) {
                        return Err(unsupported_type(&p, &name, value));
                    }
                    request.params = Some(read_val(&mut p, 0)?);
                }
                "id" => {
                    if value != Some(Kind::String) {
                        return Err(unsupported_type(&p, &name, value));
                    }
                    // ALLOC: the template, its params and its output, which the request limits bound.
                    request.id = Some(p.text().to_owned());
                    request.stored = true;
                }
                "source" | "inline" | "template" => match value {
                    // ALLOC: the template, its params and its output, which the request limits bound.
                    Some(Kind::String) => {
                        request.source = Some(p.text().to_owned());
                        request.stored = false;
                    }
                    Some(Kind::StartObject) => {
                        // ALLOC: the template, its params and its output, which the request limits bound.
                        let mut json = String::new();
                        read_val(&mut p, 0)?.write_json(&mut json);
                        request.source = Some(json);
                        request.stored = false;
                    }
                    _ => return Err(unsupported_type(&p, &name, value)),
                },
                "explain" | "profile" => {
                    if value != Some(Kind::Bool) {
                        return Err(unsupported_type(&p, &name, value));
                    }
                    let on = p.text() == "true";
                    if name == "explain" {
                        request.explain = on;
                    } else {
                        request.profile = on;
                    }
                }
                "search_pipeline" => match value {
                    Some(Kind::Null) => request.search_pipeline = false,
                    Some(Kind::String) => request.search_pipeline = !p.text().is_empty(),
                    _ => return Err(unsupported_type(&p, &name, value)),
                },
                _ => {
                    return Err(DslError::xcontent(
                        name_at,
                        // ALLOC: error path; one message.
                        &format!("[search_template] unknown field [{name}]"),
                        None,
                    ));
                }
            }
        }
        Ok(request)
    }
}

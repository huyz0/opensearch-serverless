//! `function_score`'s `script_score` over `lang: expression`: the script as
//! `ScriptScoreFunctionBuilder.fromXContent` reads it, and the engine's
//! expression-script function built from it.
//!
//! An expression is loop-free, so a script costs its source size and nothing
//! it runs allocates per document. Painless source is refused: the native
//! runtime executes the IR of Java's compiler, which a node does not have.

use std::sync::Arc;

use lucene_engine::function_score::Function as Engine;
use lucene_engine::painless_score::{EXPR_SCRIPT, ScriptFunction};
use lucene_engine::query_blob::Cursor;
use lucene_expressions::js::compile;
use lucene_expressions::variable_context::{Type, parse as parse_variable};

use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::failure::Failure;
use crate::lucene_query::QueryContext;
use crate::mapping::FieldType;

/// The most a script's source is, as core's `script.max_size_in_bytes`.
const MAX_SOURCE_BYTES: usize = 65_535;

/// A parsed `script_score` script.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptScore {
    /// The expression's source.
    pub source: String,
    /// The numeric params, by name.
    pub params: Vec<(String, f64)>,
}

fn refuse(what: &str) -> DslError {
    // ALLOC: error path; one message.
    DslError::unsupported(&format!("[script_score] {what}"))
}

/// `ScriptScoreFunctionBuilder.fromXContent`: on the function object's start.
pub fn parse(p: &mut Parser<'_>) -> Result<ScriptScore, DslError> {
    let mut script = None;
    while p.next_token() == Some(Kind::FieldName) {
        // ALLOC: bounded by the script's source, at most 65,535 bytes.
        let name = p.text().to_owned();
        match (name.as_str(), p.next_token()) {
            ("script", Some(Kind::String)) => {
                return Err(DslError::illegal(
                    "[script_score] runs only lang [expression] on this node, and a script given as text is painless",
                ));
            }
            ("script", Some(Kind::StartObject)) => script = Some(object(p)?),
            _ => {
                // ALLOC: error path; one message.
                return Err(DslError::illegal(format!(
                    "[script_score] unknown field [{name}]"
                )));
            }
        }
    }
    script.ok_or_else(|| DslError::illegal("[script_score] requires a [script]"))
}

fn object(p: &mut Parser<'_>) -> Result<ScriptScore, DslError> {
    // ALLOC: bounded by the script's source, at most 65,535 bytes.
    let (mut source, mut lang, mut params) = (None, None, Vec::new());
    while p.next_token() == Some(Kind::FieldName) {
        // ALLOC: bounded by the script's source, at most 65,535 bytes.
        let name = p.text().to_owned();
        match (name.as_str(), p.next_token()) {
            // ALLOC: bounded by the script's source, at most 65,535 bytes.
            ("source" | "inline", Some(Kind::String)) => source = Some(p.text().to_owned()),
            // ALLOC: bounded by the script's source, at most 65,535 bytes.
            ("lang", Some(Kind::String)) => lang = Some(p.text().to_owned()),
            ("params", Some(Kind::StartObject)) => {
                while p.next_token() == Some(Kind::FieldName) {
                    // ALLOC: bounded by the script's source, at most 65,535 bytes.
                    let key = p.text().to_owned();
                    match p.next_token() {
                        Some(Kind::Number) => {
                            let v = p.text().parse::<f64>().map_err(|_| {
                                DslError::illegal("[script_score] a param is not a number")
                            })?;
                            params.push((key, v));
                        }
                        _ => return Err(refuse("a param that is not a number")),
                    }
                }
            }
            ("id", _) => return Err(refuse("a stored script [id]")),
            ("options", _) => return Err(refuse("script [options]")),
            _ => {
                // ALLOC: error path; one message.
                return Err(DslError::illegal(format!(
                    "[script_score] unknown script field [{name}]"
                )));
            }
        }
    }
    if lang.as_deref() != Some("expression") {
        return Err(DslError::illegal(
            "[script_score] runs only lang [expression] on this node",
        ));
    }
    let source =
        source.ok_or_else(|| DslError::illegal("[script_score] script requires a [source]"))?;
    if source.len() > MAX_SOURCE_BYTES {
        return Err(DslError::illegal(
            "[script_score] source is over the 65535 byte limit",
        ));
    }
    Ok(ScriptScore { source, params })
}

fn script_error(reason: String) -> Failure {
    Failure::plain("script_exception", reason)
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    let n = i32::try_from(b.len()).unwrap_or(i32::MAX);
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(b);
}

/// The engine function for `script` over the mapping in `cx`.
pub fn build(cx: &QueryContext<'_>, script: &ScriptScore) -> Result<Engine, Failure> {
    // ALLOC: bounded by the script's source, at most 65,535 bytes.
    let expr = compile(&script.source).map_err(|e| script_error(format!("compile error: {e}")))?;
    // ALLOC: one name a field the script reads, bounded by the source.
    let mut fields: Vec<String> = Vec::new();
    // ALLOC: one binding a variable, bounded by the source.
    let mut bindings: Vec<u8> = Vec::new();
    for name in &expr.variables {
        put_bytes(&mut bindings, name.as_bytes());
        if name == "_score" {
            bindings.push(0);
            continue;
        }
        // ALLOC: bounded by the script's source, at most 65,535 bytes.
        let parts = parse_variable(name).map_err(|e| script_error(e.to_string()))?;
        let text = |i: usize| parts.get(i).and_then(|c| c.text.as_deref());
        match (text(0), parts.first().map(|c| c.kind)) {
            (Some(key), _) if key != "doc" => {
                let Some((_, v)) = script.params.iter().find(|(k, _)| k == key) else {
                    // ALLOC: bounded by the script's source, at most 65,535 bytes.
                    return Err(script_error(format!(
                        "Variable [{name}] is not a number param or a doc value"
                    )));
                };
                bindings.push(1);
                bindings.extend_from_slice(&v.to_bits().to_le_bytes());
            }
            (Some("doc"), _) if parts.get(1).map(|c| c.kind) == Some(Type::StrIndex) => {
                let field = text(1).unwrap_or_default();
                match cx.mapping.get(field).map(|f| &f.ty) {
                    Some(FieldType::Int { .. } | FieldType::Long | FieldType::Date) => {}
                    Some(_) => {
                        // ALLOC: bounded by the script's source, at most 65,535 bytes.
                        return Err(script_error(format!(
                            "[{field}] is not a number or a date field, which an expression reads"
                        )));
                    }
                    None => {
                        // ALLOC: bounded by the script's source, at most 65,535 bytes.
                        return Err(script_error(format!(
                            "No field found for [{field}] in mapping"
                        )));
                    }
                }
                let at = fields.iter().position(|f| f == field).unwrap_or_else(|| {
                    // ALLOC: one name a field the script reads.
                    fields.push(field.to_owned());
                    fields.len().saturating_sub(1)
                });
                let at = i32::try_from(at).unwrap_or(0).to_le_bytes();
                match text(2) {
                    Some(m @ ("value" | "min" | "max" | "avg" | "median" | "sum")) => {
                        bindings.push(2);
                        bindings.extend_from_slice(&at);
                        bindings.push(0); // numeric: long
                        bindings.push(match m {
                            "value" | "min" => 0,
                            "max" => 1,
                            "avg" => 2,
                            "median" => 3,
                            _ => 4,
                        });
                    }
                    Some("empty") => {
                        bindings.push(3);
                        bindings.extend_from_slice(&at);
                    }
                    Some("length") => {
                        bindings.push(4);
                        bindings.extend_from_slice(&at);
                    }
                    _ => {
                        // ALLOC: bounded by the script's source, at most 65,535 bytes.
                        return Err(script_error(format!(
                            "Variable [{name}] is not served: only value, min, max, avg, median, sum, empty and length of a field"
                        )));
                    }
                }
            }
            _ => {
                // ALLOC: bounded by the script's source, at most 65,535 bytes.
                return Err(script_error(format!("Unknown variable [{name}]")));
            }
        }
    }
    let needs_score = expr.variables.iter().any(|v| v == "_score");
    // ALLOC: one blob a script, bounded by the source and its variables.
    let mut blob = vec![EXPR_SCRIPT | u8::from(needs_score)];
    put_bytes(&mut blob, script.source.as_bytes());
    // The params are bound as constants; the map the spec holds is empty.
    put_bytes(&mut blob, &[12, 0, 0, 0, 0]);
    put_bytes(&mut blob, b"");
    blob.extend_from_slice(&0i32.to_le_bytes());
    blob.extend_from_slice(&0i32.to_le_bytes());
    blob.extend_from_slice(&i32::try_from(fields.len()).unwrap_or(0).to_le_bytes());
    for f in &fields {
        put_bytes(&mut blob, f.as_bytes());
        put_bytes(&mut blob, f.as_bytes());
        blob.push(0);
    }
    blob.extend_from_slice(
        &i32::try_from(expr.variables.len())
            .unwrap_or(0)
            .to_le_bytes(),
    );
    blob.extend_from_slice(&bindings);
    let mut cursor = Cursor::new(&blob);
    let function = ScriptFunction::decode(&mut cursor)
        // ALLOC: bounded by the script's source, at most 65,535 bytes.
        .map_err(|_| script_error("the expression could not be set up".to_owned()))?;
    Ok(Engine::Script(Arc::new(function)))
}

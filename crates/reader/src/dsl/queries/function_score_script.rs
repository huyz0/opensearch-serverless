//! `function_score`'s `script_score` over `lang: expression`: the script as
//! `ScriptScoreFunctionBuilder.fromXContent` reads it, and the engine's
//! expression-script function built from it.
//!
//! An expression is loop-free, so a script costs its source size and nothing
//! it runs allocates per document. Painless source is compiled by the Rust
//! front end when the node's `native_compile` switch is on
//! ([`crate::painless_switch`]) and refused as an expression-only node does
//! otherwise.

use std::sync::Arc;

use lucene_engine::function_score::Function as Engine;
use lucene_engine::painless_score::{EXPR_SCRIPT, PainlessSort, ScriptFunction};
use lucene_engine::query_blob::Cursor;
use lucene_expressions::js::compile;
use lucene_expressions::variable_context::{Type, parse as parse_variable};
use lucene_painless::ir::Constant;

use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::failure::Failure;
use crate::lucene_query::QueryContext;
use crate::mapping::{FieldType, Mapping};

/// The most a script's source is, as core's `script.max_size_in_bytes`.
const MAX_SOURCE_BYTES: usize = 65_535;

/// A script param's value, as the request's JSON gives it.
#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    /// A number without a fraction or exponent that fits an `int`.
    Int(i32),
    /// A number without a fraction or exponent that fits a `long`.
    Long(i64),
    /// Any other number.
    Double(f64),
    /// A string.
    Str(String),
    /// A boolean.
    Bool(bool),
}

impl Param {
    /// The number an expression reads, none for a string or a boolean.
    fn number(&self) -> Option<f64> {
        match self {
            // Lossless: an int is exact in a double; a long may round, as a
            // number does when the expression's JSON is read as a double.
            Param::Int(v) => Some(f64::from(*v)),
            #[allow(clippy::cast_precision_loss)]
            Param::Long(v) => Some(*v as f64),
            Param::Double(v) => Some(*v),
            Param::Str(_) | Param::Bool(_) => None,
        }
    }

    pub(crate) fn constant(&self) -> Constant {
        match self {
            Param::Int(v) => Constant::Int(*v),
            Param::Long(v) => Constant::Long(*v),
            Param::Double(v) => Constant::Double(*v),
            // ALLOC: bounded by the request body.
            Param::Str(v) => Constant::Str(v.clone()),
            Param::Bool(v) => Constant::Bool(*v),
        }
    }

    fn of_number(text: &str) -> Option<Param> {
        if !text.contains(['.', 'e', 'E']) {
            if let Ok(v) = text.parse::<i32>() {
                return Some(Param::Int(v));
            }
            if let Ok(v) = text.parse::<i64>() {
                return Some(Param::Long(v));
            }
        }
        text.parse::<f64>().ok().map(Param::Double)
    }
}

/// The language a script is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    /// `expression`.
    Expression,
    /// `painless`, compiled by this node.
    Painless,
}

/// A parsed `script_score` script.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptScore {
    /// The script's source.
    pub source: String,
    /// The language.
    pub lang: Lang,
    /// The params, by name.
    pub params: Vec<(String, Param)>,
}

fn refuse(what: &str) -> DslError {
    // ALLOC: error path; one message.
    DslError::unsupported(&format!("[script] {what}"))
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
            ("script", Some(Kind::StartObject)) => {
                script = Some(object_with(p, crate::painless_switch::native_compile())?);
            }
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

/// A script's `params` object, the parser on its start: numbers, strings
/// and booleans.
pub(crate) fn parse_params(p: &mut Parser<'_>) -> Result<Vec<(String, Param)>, DslError> {
    // ALLOC: one entry a param, bounded by the request body.
    let mut params = Vec::new();
    while p.next_token() == Some(Kind::FieldName) {
        // ALLOC: bounded by the script's source, at most 65,535 bytes.
        let key = p.text().to_owned();
        match p.next_token() {
            Some(Kind::Number) => {
                let v = Param::of_number(p.text())
                    .ok_or_else(|| DslError::illegal("[script] a param is not a number"))?;
                params.push((key, v));
            }
            // ALLOC: bounded by the script's source, at most 65,535 bytes.
            Some(Kind::String) => params.push((key, Param::Str(p.text().to_owned()))),
            Some(Kind::Bool) => params.push((key, Param::Bool(p.text() == "true"))),
            _ => return Err(refuse("a param that is not a number")),
        }
    }
    Ok(params)
}

/// A script object, on its start; `painless` says whether `lang: painless`
/// (the default when `lang` is absent) is served.
pub fn object_with(p: &mut Parser<'_>, painless: bool) -> Result<ScriptScore, DslError> {
    // ALLOC: bounded by the script's source, at most 65,535 bytes.
    let (mut source, mut lang, mut params, mut id) = (None, None, Vec::new(), None);
    while p.next_token() == Some(Kind::FieldName) {
        // ALLOC: bounded by the script's source, at most 65,535 bytes.
        let name = p.text().to_owned();
        match (name.as_str(), p.next_token()) {
            // ALLOC: bounded by the script's source, at most 65,535 bytes.
            ("source" | "inline", Some(Kind::String)) => source = Some(p.text().to_owned()),
            // ALLOC: bounded by the script's source, at most 65,535 bytes.
            ("lang", Some(Kind::String)) => lang = Some(p.text().to_owned()),
            ("params", Some(Kind::StartObject)) => params = parse_params(p)?,
            // ALLOC: bounded by the id, at most the body.
            ("id", Some(Kind::String)) => id = Some(p.text().to_owned()),
            ("id", _) => return Err(DslError::illegal("[script] [id] must be a string")),
            ("options", _) => return Err(refuse("script [options]")),
            _ => {
                // ALLOC: error path; one message.
                return Err(DslError::illegal(format!(
                    "[script] unknown script field [{name}]"
                )));
            }
        }
    }
    if let Some(id) = id {
        let (resolved, stored_lang) = stored(&id, source.as_deref(), lang.as_deref())?;
        source = Some(resolved);
        lang = Some(stored_lang);
    }
    let lang = match lang.as_deref() {
        Some("expression") => Lang::Expression,
        None | Some("painless") if painless => Lang::Painless,
        _ => {
            return Err(DslError::illegal(
                "[script] runs only lang [expression] on this node",
            ));
        }
    };
    if lang == Lang::Expression && params.iter().any(|(_, v)| v.number().is_none()) {
        return Err(refuse("a param that is not a number"));
    }
    let source = source.ok_or_else(|| DslError::illegal("[script] script requires a [source]"))?;
    if source.len() > MAX_SOURCE_BYTES {
        return Err(DslError::illegal(
            "[script] source is over the 65535 byte limit",
        ));
    }
    Ok(ScriptScore {
        source,
        lang,
        params,
    })
}

/// The source and language of the stored script `id`, which a script object
/// names without a `source` or a `lang`.
fn stored(
    id: &str,
    source: Option<&str>,
    lang: Option<&str>,
) -> Result<(String, String), DslError> {
    use crate::stored_scripts::{ResolveError, installed};
    if source.is_some() || lang.is_some() {
        return Err(DslError::illegal(
            "[script] a stored script [id] takes no [source] or [lang]",
        ));
    }
    let Some(scripts) = installed() else {
        return Err(refuse(
            "a stored script [id] is not served: send the script as [source]",
        ));
    };
    match scripts.resolve(id) {
        Ok(found) if matches!(found.lang.as_str(), "painless" | "expression") => {
            Ok((found.source, found.lang))
        }
        // ALLOC: error path; one message.
        Ok(found) => Err(DslError::illegal(format!(
            "[script] the stored script [{id}] is lang [{}], which this node does not run here",
            found.lang
        ))),
        Err(ResolveError::Missing) => Err(DslError::Failure(Failure::plain(
            "resource_not_found_exception",
            // ALLOC: error path; one message.
            format!("unable to find script [{id}] in cluster state"),
        ))),
        Err(ResolveError::TooBig(m)) => Err(DslError::Failure(Failure::plain(
            "circuit_breaking_exception",
            m,
        ))),
        Err(ResolveError::Failed(m)) => Err(DslError::Failure(Failure::plain("exception", m))),
    }
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
    let blob = encode(cx.mapping, script, 0)?;
    let function = ScriptFunction::decode(&mut Cursor::new(&blob))
        // ALLOC: bounded by the script's source, at most 65,535 bytes.
        .map_err(|_| script_error("the expression could not be set up".to_owned()))?;
    Ok(Engine::Script(Arc::new(function)))
}

/// The engine's script for one hit's field (`script_fields`): the
/// expression's number for a document, never reading `_score`.
pub fn field(mapping: &Mapping, script: &ScriptScore) -> Result<Arc<ScriptFunction>, Failure> {
    let blob = encode(mapping, script, 0)?;
    let function = ScriptFunction::decode(&mut Cursor::new(&blob))
        // ALLOC: bounded by the request body's recording.
        .map_err(|_| script_error("the expression could not be set up".to_owned()))?;
    if function.needs_score() {
        return Err(Failure::plain(
            "illegal_argument_exception",
            "a [script_fields] script that reads [_score] is not supported by this node yet",
        ));
    }
    Ok(Arc::new(function))
}

/// The engine's sort over `script`: its number for every document.
pub fn sort(cx: &QueryContext<'_>, script: &ScriptScore) -> Result<Arc<PainlessSort>, Failure> {
    let blob = encode(cx.mapping, script, 3)?;
    let sort = PainlessSort::decode(&mut Cursor::new(&blob))
        // ALLOC: bounded by the script's source, at most 65,535 bytes.
        .map_err(|_| script_error("the expression could not be set up".to_owned()))?;
    Ok(Arc::new(sort))
}

/// The engine's blob for `script`: `context` is the engine's script context
/// (`0` score, `3` number sort), plus one when the script reads `_score`.
fn encode(mapping: &Mapping, script: &ScriptScore, context: u8) -> Result<Vec<u8>, Failure> {
    if script.lang == Lang::Painless {
        return super::function_score_painless::encode(mapping, script, context);
    }
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
                let Some(v) = script
                    .params
                    .iter()
                    .find(|(k, _)| k == key)
                    .and_then(|(_, v)| v.number())
                else {
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
                match mapping.get(field).map(|f| &f.ty) {
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
    let mut blob = vec![EXPR_SCRIPT | context | u8::from(needs_score)];
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
    Ok(blob)
}

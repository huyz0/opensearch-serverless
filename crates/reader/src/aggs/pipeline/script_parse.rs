//! The bodies of `bucket_script` (`BucketScriptPipelineAggregationBuilder`'s
//! `ConstructingObjectParser`) and `bucket_selector` (a hand-written loop
//! over the tokens), each with its own messages.

use lucene_painless::ir::Constant;

use super::parent_parse::{bucket_path, invalid_gap, pipeline};
use super::script::ScriptPipe;
use super::{BucketValue, PipelineAgg, PipelineKind};
use crate::aggs::metric::script_values;
use crate::dsl::error::DslError;
use crate::dsl::queries::function_score_script::{Lang, ScriptScore};
use crate::dsl::tokens::{Kind, Parser};
use crate::failure::Failure;

/// Reads the body of a `bucket_script` or `bucket_selector` named `name`,
/// the parser on its start object.
pub(super) fn parse(
    p: &mut Parser<'_>,
    name: &str,
    kind: PipelineKind,
) -> Result<PipelineAgg, DslError> {
    let (paths, script, insert_zeros) = if kind == PipelineKind::BucketScript {
        script_body(p)?
    } else {
        selector_body(p, name)?
    };
    let mut vars = Vec::with_capacity(paths.len()); // ALLOC: one entry a variable, bounded by the request.
    for (var, path) in paths {
        let (_, value) = bucket_path(&path, name, kind)?;
        if matches!(value, BucketValue::Other { .. } | BucketValue::Key) {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!(
                "the [buckets_path] [{path}] of the [{}] aggregation [{name}]",
                kind.name()
            )));
        }
        vars.push((var, value));
    }
    if script.lang != Lang::Painless {
        // ALLOC: error path; one message.
        return Err(DslError::unsupported(&format!(
            "the [script] of a [{}] aggregation other than painless",
            kind.name()
        )));
    }
    // ALLOC: one script's source and params, bounded by the request body.
    let mut agg = pipeline(name, kind, String::new(), insert_zeros);
    // ALLOC: as above.
    agg.script = Some(Box::new(ScriptPipe {
        source: script.source,
        params: script
            .params
            .iter()
            // ALLOC: one entry a param, bounded by the request body.
            .map(|(k, v)| (k.clone(), v.constant()))
            // ALLOC: as above.
            .collect::<Vec<(String, Constant)>>(),
        vars,
        window: 0,
        shift: 0,
    }));
    Ok(agg)
}

/// A variable's name and its `buckets_path`.
type Paths = Vec<(String, String)>;

/// The text of the current scalar token of a `buckets_path` object's value,
/// `parser.text()`.
fn text_of(p: &Parser<'_>, token: Kind) -> Option<String> {
    matches!(token, Kind::String | Kind::Number | Kind::Bool).then(|| p.text().to_owned()) // ALLOC: one path.
}

/// The `buckets_path` object, after its `{`, the way `bucket_script`'s
/// parser reads each value as text.
fn script_paths(p: &mut Parser<'_>) -> Result<Paths, DslError> {
    let mut paths = Vec::new(); // ALLOC: one entry a variable, bounded by the request.
    let mut var = String::new(); // ALLOC: one name.
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => return Ok(paths),
            Some(Kind::FieldName) => var = p.text().to_owned(), // ALLOC: one name.
            Some(token) => {
                let Some(path) = text_of(p, token) else {
                    let at = p.location();
                    return Err(DslError::xcontent(
                        at,
                        "[bucket_script] failed to parse field [buckets_path]",
                        Some(DslError::Failure(Failure::plain(
                            "illegal_state_exception",
                            // ALLOC: error path; one message.
                            format!(
                                "Can't get text on a {} at {}:{}",
                                token.name(),
                                at.line,
                                at.column
                            ),
                        ))),
                    ));
                };
                paths.push((std::mem::take(&mut var), path));
            }
        }
    }
}

/// `buckets_path` as an array of paths, after its `[`: the variables are
/// `_value0`, `_value1` and so on.
fn array_paths(p: &mut Parser<'_>, who: &str) -> Result<Paths, DslError> {
    let mut paths = Vec::new(); // ALLOC: one entry a variable, bounded by the request.
    loop {
        match p.next_token() {
            Some(Kind::EndArray) | None => return Ok(paths),
            Some(token) => {
                let Some(path) = text_of(p, token) else {
                    // ALLOC: error path; one message.
                    return Err(DslError::unsupported(&format!(
                        "a [buckets_path] of a [{who}] aggregation that is not text"
                    )));
                };
                // ALLOC: one name a variable.
                paths.push((format!("_value{}", paths.len()), path));
            }
        }
    }
}

/// `buckets_path` as one path: the variable is `_value`.
fn single_path(p: &Parser<'_>) -> Paths {
    // ALLOC: one entry.
    vec![("_value".to_owned(), p.text().to_owned())]
}

fn script_body(p: &mut Parser<'_>) -> Result<(Paths, ScriptScore, bool), DslError> {
    let (mut paths, mut script, mut insert_zeros) = (None, None, false);
    let mut key = String::new(); // ALLOC: one key.
    let mut key_at = p.location();
    loop {
        let token = match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                key = p.text().to_owned(); // ALLOC: one key.
                key_at = p.location();
                continue;
            }
            Some(token) => token,
        };
        let at = p.location();
        let wrong_type = |field: &str| {
            DslError::xcontent(
                at,
                // ALLOC: error path; one message.
                &format!(
                    "[bucket_script] {field} doesn't support values of type: {}",
                    token.name()
                ),
                None,
            )
        };
        match (key.as_str(), token) {
            ("buckets_path", Kind::StartObject) => paths = Some(script_paths(p)?),
            ("buckets_path", Kind::StartArray) => paths = Some(array_paths(p, "bucket_script")?),
            ("buckets_path", Kind::String) => paths = Some(single_path(p)),
            ("buckets_path", _) => return Err(wrong_type("buckets_path")),
            ("script", Kind::String | Kind::StartObject) => {
                script = Some(script_values::parse(p, token)?);
            }
            ("script", _) => return Err(wrong_type("script")),
            ("gap_policy", Kind::String) => match p.text() {
                "skip" => insert_zeros = false,
                "insert_zeros" => insert_zeros = true,
                other => {
                    return Err(DslError::xcontent(
                        at,
                        "[bucket_script] failed to parse field [gap_policy]",
                        Some(invalid_gap(p, other)),
                    ));
                }
            },
            ("gap_policy", _) => return Err(wrong_type("gap_policy")),
            ("format", _) => {
                return Err(DslError::unsupported(
                    "the [format] of a [bucket_script] aggregation",
                ));
            }
            _ => {
                return Err(DslError::xcontent(
                    key_at,
                    // ALLOC: error path; one message.
                    &format!("[bucket_script] unknown field [{key}]"),
                    None,
                ));
            }
        }
    }
    match (paths, script) {
        (Some(paths), Some(script)) => Ok((paths, script, insert_zeros)),
        (None, Some(_)) => Err(DslError::illegal("Required [buckets_path]")),
        (Some(_), None) => Err(DslError::illegal("Required [script]")),
        (None, None) => Err(DslError::illegal("Required [buckets_path, script]")),
    }
}

/// `BucketSelectorPipelineAggregationBuilder.parse`.
fn selector_body(p: &mut Parser<'_>, name: &str) -> Result<(Paths, ScriptScore, bool), DslError> {
    let (mut paths, mut script, mut insert_zeros) = (None, None, false);
    let mut key = String::new(); // ALLOC: one key.
    loop {
        let token = match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                key = p.text().to_owned(); // ALLOC: one key.
                continue;
            }
            Some(token) => token,
        };
        match (token, key.as_str()) {
            (Kind::String, "buckets_path") => paths = Some(single_path(p)),
            (Kind::String, "gap_policy") => match p.text() {
                "skip" => insert_zeros = false,
                "insert_zeros" => insert_zeros = true,
                other => return Err(invalid_gap(p, other)),
            },
            (Kind::String | Kind::StartObject, "script") => {
                script = Some(script_values::parse(p, token)?);
            }
            (Kind::StartArray, "buckets_path") => paths = Some(array_paths(p, "bucket_selector")?),
            (Kind::StartObject, "buckets_path") => paths = Some(selector_paths(p)?),
            (Kind::String | Kind::StartArray | Kind::StartObject, _) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!("Unknown key for a {} in [{name}]: [{key}].", token.name()),
                ));
            }
            (other, _) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!("Unexpected token {} in [{name}].", other.name()),
                ));
            }
        }
    }
    let missing = |field: &str| {
        DslError::parsing(
            p.location(),
            // ALLOC: error path; one message.
            format!("Missing required field [{field}] for bucket_selector aggregation [{name}]"),
        )
    };
    let paths = paths.ok_or_else(|| missing("buckets_path"))?;
    let script = script.ok_or_else(|| missing("script"))?;
    Ok((paths, script, insert_zeros))
}

/// The selector's `buckets_path` object: a value is the token's text, a
/// `null` the text `null`.
fn selector_paths(p: &mut Parser<'_>) -> Result<Paths, DslError> {
    let mut paths = Vec::new(); // ALLOC: one entry a variable, bounded by the request.
    let mut var = String::new(); // ALLOC: one name.
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => return Ok(paths),
            Some(Kind::FieldName) => var = p.text().to_owned(), // ALLOC: one name.
            Some(Kind::Null) => paths.push((std::mem::take(&mut var), "null".to_owned())), // ALLOC: one path.
            Some(token) => {
                let Some(path) = text_of(p, token) else {
                    return Err(DslError::unsupported(
                        "a [buckets_path] of a [bucket_selector] aggregation that is not text",
                    ));
                };
                paths.push((std::mem::take(&mut var), path));
            }
        }
    }
}

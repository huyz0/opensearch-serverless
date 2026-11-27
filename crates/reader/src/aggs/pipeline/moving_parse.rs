//! The body of `moving_fn`: `MovFnPipelineAggregationBuilder.PARSER`, a
//! `ConstructingObjectParser` over `buckets_path`, `script` and `window`
//! (all required) with `gap_policy`, `shift` and `format`.

use super::parent_parse::{bucket_path, invalid_gap, pipeline};
use super::script::ScriptPipe;
use super::{BucketValue, PipelineAgg, PipelineKind};
use crate::aggs::metric::script_values;
use crate::dsl::error::DslError;
use crate::dsl::queries::function_score_script::{Lang, ScriptScore};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::int_value;
use crate::failure::Failure;

const WHO: &str = "[moving_fn]";

/// Reads the body of a `moving_fn` named `name`, the parser on its start
/// object.
pub(super) fn parse(p: &mut Parser<'_>, name: &str) -> Result<PipelineAgg, DslError> {
    let kind = PipelineKind::MovingFn;
    let mut path: Option<String> = None;
    let mut script: Option<ScriptScore> = None;
    let (mut window, mut shift, mut insert_zeros) = (None, 0_i32, false);
    let mut built = false;
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
                    "{WHO} {field} doesn't support values of type: {}",
                    token.name()
                ),
                None,
            )
        };
        let failed_field = |field: &str, cause: DslError| {
            DslError::xcontent(
                at,
                // ALLOC: error path; one message.
                &format!("{WHO} failed to parse field [{field}]"),
                Some(cause),
            )
        };
        match (key.as_str(), token) {
            ("buckets_path", Kind::String) => path = Some(p.text().to_owned()), // ALLOC: one path.
            ("buckets_path", _) => return Err(wrong_type("buckets_path")),
            ("script", Kind::String | Kind::StartObject) => {
                script = Some(script_values::parse(p, token)?);
            }
            ("script", _) => return Err(wrong_type("script")),
            ("window", Kind::Number | Kind::String) => {
                window = Some(int_value(p).map_err(|e| failed_field("window", e))?);
            }
            ("window", _) => return Err(wrong_type("window")),
            ("shift", Kind::Number | Kind::String) => {
                shift = int_value(p).map_err(|e| failed_field("shift", e))?;
            }
            ("shift", _) => return Err(wrong_type("shift")),
            ("gap_policy", Kind::String) => match p.text() {
                "skip" => insert_zeros = false,
                "insert_zeros" => insert_zeros = true,
                other => return Err(failed_field("gap_policy", invalid_gap(p, other))),
            },
            ("gap_policy", _) => return Err(wrong_type("gap_policy")),
            ("format", _) => {
                return Err(DslError::unsupported(
                    "the [format] of a [moving_fn] aggregation",
                ));
            }
            _ => {
                return Err(DslError::xcontent(
                    key_at,
                    // ALLOC: error path; one message.
                    &format!("{WHO} unknown field [{key}]"),
                    None,
                ));
            }
        }
        // The builder is made when the last required field arrives.
        if !built && path.is_some() && script.is_some() && window.is_some() {
            built = true;
            if window.is_some_and(|w| w < 1) {
                let cause = Failure::plain(
                    "x_content_parse_exception",
                    "Failed to build [moving_fn] after last required field arrived",
                )
                .caused_by(Failure::plain(
                    "illegal_argument_exception",
                    "[window] must be a positive, non-zero integer.",
                ));
                return Err(DslError::xcontent(
                    at,
                    // ALLOC: error path; one message.
                    &format!("{WHO} failed to parse field [{key}]"),
                    Some(DslError::Failure(cause)),
                ));
            }
        }
    }
    // ALLOC: error path; the names of the missing fields.
    let missing: Vec<&str> = [
        ("buckets_path", path.is_none()),
        ("script", script.is_none()),
        ("window", window.is_none()),
    ]
    .into_iter()
    .filter_map(|(n, absent)| absent.then_some(n))
    // ALLOC: as above.
    .collect();
    let (Some(path), Some(script), Some(window)) = (path, script, window) else {
        // ALLOC: error path; one message.
        return Err(DslError::illegal(format!(
            "Required [{}]",
            missing.join(", ")
        )));
    };
    if script.lang != Lang::Painless {
        return Err(DslError::unsupported(
            "the [script] of a [moving_fn] aggregation other than painless",
        ));
    }
    let (_, value) = bucket_path(&path, name, kind)?;
    if matches!(value, BucketValue::Other { .. } | BucketValue::Key) {
        // ALLOC: error path; one message.
        return Err(DslError::unsupported(&format!(
            "the [buckets_path] [{path}] of the [moving_fn] aggregation [{name}]"
        )));
    }
    let mut agg = pipeline(name, kind, path, insert_zeros);
    agg.value = value;
    // ALLOC: one script's source and params, bounded by the request body.
    agg.script = Some(Box::new(ScriptPipe {
        source: script.source,
        params: script
            .params
            .iter()
            // ALLOC: one entry a param, bounded by the request body.
            .map(|(k, v)| (k.clone(), v.constant()))
            // ALLOC: as above.
            .collect(),
        vars: Vec::new(),
        window: usize::try_from(window).unwrap_or(1),
        shift: i64::from(shift),
    }));
    Ok(agg)
}

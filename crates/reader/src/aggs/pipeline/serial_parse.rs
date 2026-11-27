//! The body of `serial_diff`: `SerialDiffPipelineAggregationBuilder.parse`,
//! a hand-written loop over the tokens like `derivative`'s, with its own
//! messages (which name `derivative` for a missing `buckets_path`) and a
//! `lag`.

use super::parent_parse::{invalid_gap, pipeline};
use super::{PipelineAgg, PipelineKind};
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::int_value;

/// Reads the body of a `serial_diff` named `name`, after its `{`.
pub(super) fn serial_diff(p: &mut Parser<'_>, name: &str) -> Result<PipelineAgg, DslError> {
    let mut path: Option<String> = None;
    let mut insert_zeros = false;
    let mut lag = 1_usize;
    let mut key = "";
    loop {
        let token = match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                key = p.text();
                continue;
            }
            Some(token) => token,
        };
        match (token, key) {
            (Kind::String, "buckets_path") => path = Some(p.text().to_owned()), // ALLOC: one path.
            (Kind::String, "gap_policy") => match p.text() {
                "skip" => insert_zeros = false,
                "insert_zeros" => insert_zeros = true,
                other => return Err(invalid_gap(p, other)),
            },
            (Kind::String, "format") => {
                return Err(DslError::unsupported(
                    "the [format] of a [serial_diff] aggregation",
                ));
            }
            (Kind::Number, "lag") => {
                if p.text()
                    .parse::<i64>()
                    .is_ok_and(|n| i32::try_from(n).is_err())
                {
                    return Err(DslError::unsupported("a [lag] beyond 2^31"));
                }
                let n = int_value(p)?;
                lag = usize::try_from(n).ok().filter(|n| *n > 0).ok_or_else(|| {
                    DslError::parsing(
                        p.location(),
                        // ALLOC: error path; one message.
                        format!(
                            "Lag must be a positive, non-zero integer.  Value supplied was{n} in [{name}]: [lag]."
                        ),
                    )
                })?;
            }
            (Kind::StartArray, "buckets_path") => path = Some(first_path(p)?),
            (Kind::String | Kind::Number | Kind::StartArray, _) => {
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
    let Some(path) = path else {
        return Err(DslError::parsing(
            p.location(),
            // ALLOC: error path; one message.
            format!("Missing required field [buckets_path] for derivative aggregation [{name}]"),
        ));
    };
    let mut agg = pipeline(name, PipelineKind::SerialDiff, path, insert_zeros);
    agg.lag = lag;
    Ok(agg)
}

/// The first of an array of `buckets_path`; Java ignores the rest.
fn first_path(p: &mut Parser<'_>) -> Result<String, DslError> {
    let mut first: Option<String> = None;
    loop {
        match p.next_token() {
            Some(Kind::EndArray) | None => break,
            Some(Kind::String | Kind::Number) => {
                if first.is_none() {
                    first = Some(p.text().to_owned()); // ALLOC: one path.
                }
            }
            Some(_) => {
                return Err(DslError::unsupported(
                    "a [buckets_path] of a [serial_diff] aggregation that is not text",
                ));
            }
        }
    }
    first.ok_or_else(|| {
        DslError::unsupported("an empty [buckets_path] of a [serial_diff] aggregation")
    })
}

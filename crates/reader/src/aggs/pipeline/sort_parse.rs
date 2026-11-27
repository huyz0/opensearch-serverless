//! A `bucket_sort`'s body: `BucketSortPipelineAggregationBuilder.PARSER`, whose
//! messages come from `ConstructingObjectParser` and `ObjectParser`.

use crate::aggs::pipeline::parent_parse::{bucket_path, invalid_gap, pipeline};
use crate::aggs::pipeline::{BucketValue, PipelineAgg, PipelineKind, SortSpec};
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{int_value, suggest};
use crate::failure::Failure;
use oss_rest::json::Location;

/// A request path as written.
type Path = String;

const SORT_FAILED: &str = "[bucket_sort] failed to parse field [sort]";

/// A `from` or `size` whose value the builder refuses, held until the
/// builder is built: the field, its message and where its value is.
struct Queued {
    field: &'static str,
    reason: String,
    at: Location,
}

/// `BucketSortPipelineAggregationBuilder.PARSER`: a `ConstructingObjectParser`
/// whose one constructor argument, `sort`, builds the builder when it
/// arrives; `from` and `size` read before it are applied at the build, and
/// their refusals come wrapped.
pub(super) fn bucket_sort(p: &mut Parser<'_>, name: &str) -> Result<PipelineAgg, DslError> {
    const KEYS: &[&str] = &["sort", "from", "size", "gap_policy"];
    let mut spec = SortSpec::default();
    let mut insert_zeros = false;
    let mut built = false;
    let mut queued: Option<Queued> = None;
    let mut key = "";
    let mut sorts: Vec<(Path, bool)> = Vec::new(); // ALLOC: one entry a sort, bounded by the request.
    loop {
        let token = match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                key = p.text();
                if !KEYS.contains(&key) {
                    return Err(DslError::xcontent(
                        p.location(),
                        // ALLOC: error path; one message.
                        &format!("[bucket_sort] unknown field [{key}]{}", suggest(key, KEYS)),
                        None,
                    ));
                }
                continue;
            }
            Some(token) => token,
        };
        match key {
            "sort" => {
                read_sorts(p, token, &mut sorts)?;
                built = true;
                if let Some(held) = queued.take() {
                    return Err(failed_build(&held));
                }
            }
            "gap_policy" => {
                if token != Kind::String {
                    return Err(unsupported_type(p, "gap_policy", token));
                }
                insert_zeros = match p.text().to_ascii_lowercase().as_str() {
                    "skip" => false,
                    "insert_zeros" => true,
                    _ => {
                        let at = p.location();
                        let cause = invalid_gap(p, p.text());
                        return Err(DslError::xcontent(
                            at,
                            "[bucket_sort] failed to parse field [gap_policy]",
                            Some(cause),
                        ));
                    }
                };
            }
            _ => {
                if !matches!(token, Kind::Number | Kind::String) {
                    return Err(unsupported_type(p, key, token));
                }
                let at = p.location();
                let wrap = |cause: DslError| {
                    DslError::xcontent(
                        at,
                        // ALLOC: error path; one message.
                        &format!("[bucket_sort] failed to parse field [{key}]"),
                        Some(cause),
                    )
                };
                let n = int_value(p).map_err(wrap)?;
                if p.text().parse::<i64>().is_ok() && p.text().parse::<i32>().is_err() {
                    return Err(DslError::unsupported("a [from] or [size] beyond 2^31"));
                }
                let (field, refused) = if key == "from" {
                    ("from", n < 0)
                } else {
                    ("size", n <= 0)
                };
                if refused {
                    let kind = if field == "from" {
                        "non-negative"
                    } else {
                        "positive"
                    };
                    // ALLOC: error path; one message.
                    let reason = format!("[{field}] must be a {kind} integer: [{n}]");
                    if built {
                        return Err(wrap(DslError::illegal(reason)));
                    }
                    queued = Some(Queued { field, reason, at });
                }
                let n = usize::try_from(n).unwrap_or(0);
                if field == "from" {
                    spec.from = n;
                } else {
                    spec.size = Some(n);
                }
            }
        }
    }
    if let Some(held) = queued {
        return Err(failed_build(&held));
    }
    for (path, desc) in sorts {
        let (_, value) = bucket_path(&path, name, PipelineKind::BucketSort)?;
        if matches!(value, BucketValue::Other { .. }) {
            // A sort reads a bucket's own sub-aggregation, never `agg>sub`.
            // ALLOC: one entry or message bounded by the request's pipelines and names.
            return Err(DslError::unsupported(&format!(
                // ALLOC: error path; one message.
                "the sort path [{path}] of the [bucket_sort] aggregation [{name}]"
            )));
        }
        spec.fields.push((value, desc)); // ALLOC: one entry a sort.
    }
    let mut it = pipeline(name, PipelineKind::BucketSort, String::new(), insert_zeros);
    it.sort = Some(spec);
    Ok(it)
}

fn failed_build(held: &Queued) -> DslError {
    DslError::xcontent(
        held.at,
        "failed to build [bucket_sort] after last required field arrived",
        Some(DslError::xcontent(
            held.at,
            // ALLOC: error path; one message.
            &format!("[bucket_sort] failed to parse field [{}]", held.field),
            Some(DslError::illegal(held.reason.clone())),
        )),
    )
}

fn unsupported_type(p: &Parser<'_>, key: &str, token: Kind) -> DslError {
    DslError::xcontent(
        p.location(),
        // ALLOC: error path; one message.
        &format!(
            "[bucket_sort] {key} doesn't support values of type: {}",
            token.name()
        ),
        None,
    )
}

/// `sort`: an array of field names (ascending) and objects of field to
/// order, or one such object.
fn read_sorts(
    p: &mut Parser<'_>,
    token: Kind,
    sorts: &mut Vec<(Path, bool)>,
) -> Result<(), DslError> {
    match token {
        Kind::StartObject => read_sort_object(p, sorts),
        Kind::StartArray => loop {
            match p.next_token() {
                Some(Kind::EndArray) | None => return Ok(()),
                Some(Kind::String) => sorts.push((p.text().to_owned(), false)), // ALLOC: one sort.
                Some(Kind::StartObject) => read_sort_object(p, sorts)?,
                Some(_) => {
                    return Err(DslError::xcontent(
                        p.location(),
                        SORT_FAILED,
                        Some(DslError::illegal(
                            "malformed sort format, within the sort array, an object, or an actual string are allowed",
                        )),
                    ));
                }
            }
        },
        other => Err(unsupported_type(p, "sort", other)),
    }
}

/// An object of sort fields, each with its order as text.
fn read_sort_object(p: &mut Parser<'_>, sorts: &mut Vec<(Path, bool)>) -> Result<(), DslError> {
    let mut field = String::new(); // ALLOC: one name a sort.
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => return Ok(()),
            Some(Kind::FieldName) => p.text().clone_into(&mut field),
            Some(Kind::String) => {
                let desc = match p.text().to_ascii_lowercase().as_str() {
                    "asc" => false,
                    "desc" => true,
                    _ => {
                        return Err(DslError::xcontent(
                            p.location(),
                            SORT_FAILED,
                            // ALLOC: one entry or message bounded by the request's pipelines and names.
                            Some(DslError::illegal(format!(
                                // ALLOC: error path; one message.
                                "No enum constant org.opensearch.search.sort.SortOrder.{}",
                                p.text().to_ascii_uppercase()
                            ))),
                        ));
                    }
                };
                sorts.push((field.clone(), desc)); // ALLOC: one sort.
            }
            Some(_) => {
                return Err(DslError::xcontent(
                    p.location(),
                    SORT_FAILED,
                    Some(DslError::Failure(Failure::plain(
                        "x_content_parse_exception",
                        "named objects are not supported for this parser",
                    ))),
                ));
            }
        }
    }
}

//! The bodies of the parent pipeline aggregations (`derivative`,
//! `cumulative_sum` and `bucket_sort`) and the checks
//! `AggregatorFactories.Builder.validate` makes of one inside its bucket
//! aggregation.

use crate::aggs::metric::{Metric, MetricAgg};
use crate::aggs::pipeline::parse::{has_buckets, metric_builder, reads_metric, validation};
use crate::aggs::pipeline::{BucketValue, PipelineAgg, PipelineKind};
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::suggest;

/// A request path with the name it was given: the path as written.
type Path = String;

/// Reads the body of a parent pipeline of `kind` named `name`, the parser on
/// its start object.
pub fn parse_body(
    p: &mut Parser<'_>,
    name: &str,
    kind: PipelineKind,
) -> Result<PipelineAgg, DslError> {
    match kind {
        PipelineKind::Derivative => derivative(p, name),
        PipelineKind::CumulativeSum => cumulative_sum(p, name),
        PipelineKind::SerialDiff => super::serial_parse::serial_diff(p, name),
        PipelineKind::BucketScript | PipelineKind::BucketSelector => {
            super::script_parse::parse(p, name, kind)
        }
        _ => super::sort_parse::bucket_sort(p, name),
    }
}

pub(super) fn pipeline(
    name: &str,
    kind: PipelineKind,
    path: Path,
    insert_zeros: bool,
) -> PipelineAgg {
    PipelineAgg {
        // ALLOC: one name a pipeline aggregation, bounded by the body's recording.
        name: name.to_owned(),
        kind,
        path,
        // ALLOC: empty; the host's name is set when the host is known.
        agg: String::new(),
        value: BucketValue::Count,
        insert_zeros,
        host: None,
        lag: 1,
        sort: None,
        script: None,
    }
}

/// `DerivativePipelineAggregationBuilder.parse`: a hand-written loop over the
/// tokens, whose messages differ from the bucket metrics' parser.
fn derivative(p: &mut Parser<'_>, name: &str) -> Result<PipelineAgg, DslError> {
    let mut path: Option<Path> = None;
    let mut insert_zeros = false;
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
            (Kind::String, "format" | "unit") => {
                // ALLOC: error path; one message.
                return Err(DslError::unsupported(&format!(
                    "the [{key}] of a [derivative] aggregation"
                )));
            }
            (Kind::String | Kind::StartArray, _)
                if !(token == Kind::StartArray && key == "buckets_path") =>
            {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!("Unknown key for a {} in [{name}]: [{key}].", token.name()),
                ));
            }
            (Kind::StartArray, _) => {
                // Java takes the first of the paths and ignores the rest.
                let mut first: Option<Path> = None;
                loop {
                    match p.next_token() {
                        Some(Kind::EndArray) | None => break,
                        Some(Kind::String) => {
                            if first.is_none() {
                                first = Some(p.text().to_owned()); // ALLOC: one path.
                            }
                        }
                        Some(_) => {
                            return Err(DslError::unsupported(
                                "a [buckets_path] of a [derivative] aggregation that is not text",
                            ));
                        }
                    }
                }
                let Some(first) = first else {
                    return Err(DslError::unsupported(
                        "an empty [buckets_path] of a [derivative] aggregation",
                    ));
                };
                path = Some(first);
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
    Ok(pipeline(name, PipelineKind::Derivative, path, insert_zeros))
}

pub(super) fn invalid_gap(p: &Parser<'_>, given: &str) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!("Invalid gap policy: [{given}], accepted values: [insert_zeros, skip]"),
    )
}

/// `CumulativeSumPipelineAggregationBuilder.PARSER`, a
/// `ConstructingObjectParser` over `buckets_path` and `format`.
fn cumulative_sum(p: &mut Parser<'_>, name: &str) -> Result<PipelineAgg, DslError> {
    const KEYS: &[&str] = &["buckets_path", "format"];
    let mut path: Option<Path> = None;
    let mut key = "";
    loop {
        let token = match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                key = p.text();
                if !KEYS.contains(&key) {
                    return Err(DslError::xcontent(
                        p.location(),
                        // ALLOC: error path; one message.
                        &format!(
                            "[cumulative_sum] unknown field [{key}]{}",
                            suggest(key, KEYS)
                        ),
                        None,
                    ));
                }
                continue;
            }
            Some(token) => token,
        };
        match (token, key) {
            (Kind::String, "buckets_path") => path = Some(p.text().to_owned()), // ALLOC: one path.
            (Kind::String, "format") => {
                return Err(DslError::unsupported(
                    "the [format] of a [cumulative_sum] aggregation",
                ));
            }
            (Kind::StartArray, "buckets_path") => {
                // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
                let mut paths: Vec<Path> = Vec::new();
                loop {
                    match p.next_token() {
                        Some(Kind::EndArray) | None => break,
                        Some(Kind::String) => paths.push(p.text().to_owned()), // ALLOC: one path.
                        Some(_) => {
                            return Err(DslError::unsupported(
                                "a [buckets_path] of a [cumulative_sum] aggregation that is not text",
                            ));
                        }
                    }
                }
                if paths.len() != 1 {
                    // ALLOC: error path; one message.
                    return Err(DslError::unsupported(&format!(
                        "a [cumulative_sum] aggregation with {} [buckets_path]s",
                        paths.len()
                    )));
                }
                path = paths.pop();
            }
            (other, _) => {
                return Err(DslError::xcontent(
                    p.location(),
                    // ALLOC: error path; one message.
                    &format!(
                        "[cumulative_sum] {key} doesn't support values of type: {}",
                        other.name()
                    ),
                    None,
                ));
            }
        }
    }
    let Some(path) = path else {
        return Err(DslError::illegal("Required [buckets_path]"));
    };
    Ok(pipeline(name, PipelineKind::CumulativeSum, path, true))
}

/// What a path inside a bucket reads, with its first element: `_count`,
/// `_key`, a sub-aggregation's member (`value` when none is named), or
/// [`BucketValue::Other`] for a path of several elements.
pub(super) fn bucket_path(
    path: &str,
    name: &str,
    kind: PipelineKind,
) -> Result<(String, BucketValue), DslError> {
    let refuse = || {
        // ALLOC: error path; one message.
        DslError::unsupported(&format!(
            "the [buckets_path] [{path}] of the [{}] aggregation [{name}]",
            kind.name()
        ))
    };
    let mut elements = path.split('>');
    let first = elements.next().unwrap_or_default();
    if let Some(second) = elements.next() {
        // ALLOC: one name, bounded by the request.
        let second = Some(second.to_owned());
        return Ok((first.to_owned(), BucketValue::Other { second })); // ALLOC: as above.
    }
    let value = match first {
        "_count" => BucketValue::Count,
        "_key" => BucketValue::Key,
        _ => {
            let (sub, metric) = first.split_once('.').unwrap_or((first, "value"));
            if sub.is_empty() || metric.is_empty() || sub.starts_with('_') || path.contains('[') {
                return Err(refuse());
            }
            // ALLOC: one name a path, bounded by the request.
            BucketValue::Sub {
                name: sub.to_owned(),
                metric: metric.to_owned(),
            }
        }
    };
    Ok((first.to_owned(), value)) // ALLOC: as above.
}

/// Checks the parent pipelines of the aggregation `host`, as
/// `AggregatorFactories.Builder.validate` does for them in its sub-aggregations,
/// and finishes each: the host it sits in, and what it reads.
///
/// # Errors
///
/// The request's validation failure, or a refusal of a shape this node does
/// not serve.
pub fn validate_parents(host: &MetricAgg, parents: &mut [PipelineAgg]) -> Result<(), DslError> {
    // ALLOC: one name a sub-aggregation, bounded by the request's.
    let subs: Vec<String> = host.subs.iter().map(|s| s.name.clone()).collect();
    // ALLOC: one name a pipeline aggregation, bounded by the request's.
    let names: Vec<String> = parents.iter().map(|q| q.name.clone()).collect();
    for (at, pipe) in parents.iter_mut().enumerate() {
        if subs.contains(&pipe.name) || names.iter().filter(|n| **n == pipe.name).count() > 1 {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!(
                "two aggregations named [{}] under [{}]",
                pipe.name, host.name
            )));
        }
        let derived = matches!(
            pipe.kind,
            PipelineKind::Derivative | PipelineKind::CumulativeSum | PipelineKind::SerialDiff
        );
        if derived && !matches!(host.metric, Metric::Histogram | Metric::DateHistogram) {
            // ALLOC: one entry or message bounded by the request's pipelines and names.
            return Err(validation(format!(
                // ALLOC: error path; one message.
                "{} aggregation [{}] must have a histogram, date_histogram or auto_date_histogram as parent",
                pipe.kind.name(),
                pipe.name
            )));
        }
        if !has_buckets(host.metric) {
            // ALLOC: one entry or message bounded by the request's pipelines and names.
            return Err(DslError::unsupported(&format!(
                // ALLOC: error path; one message.
                "a [{}] aggregation under a [{}] aggregation",
                pipe.kind.name(),
                host.metric.name()
            )));
        }
        if derived {
            let (agg, value) = bucket_path(&pipe.path, &pipe.name, pipe.kind)?;
            check_path(
                host, &subs, &names, at, &pipe.name, &pipe.path, &agg, &value,
            )?;
            if value == BucketValue::Key {
                return Err(DslError::unsupported("a [_key] path of a parent pipeline"));
            }
            pipe.value = value;
        } else if let Some(sort) = &pipe.sort {
            for (value, _) in &sort.fields {
                check_sort(host, &subs, &names, &pipe.name, value)?;
            }
        }
        // ALLOC: one name a pipeline aggregation, bounded by the request's.
        pipe.agg.clone_from(&host.name);
        pipe.host = Some(host.name.clone()); // ALLOC: as above.
    }
    Ok(())
}

/// The path of a `derivative` or `cumulative_sum`, `split_path`'s two parts.
#[allow(clippy::too_many_arguments)]
fn check_path(
    host: &MetricAgg,
    subs: &[String],
    names: &[String],
    own: usize,
    name: &str,
    path: &str,
    agg: &str,
    value: &BucketValue,
) -> Result<(), DslError> {
    if let BucketValue::Other { second } = value {
        // `a>b`: the first element must name a sub-aggregation or pipeline.
        let known = subs.iter().any(|s| s == agg) || names.iter().any(|n| n == agg);
        if !known {
            // ALLOC: one entry or message bounded by the request's pipelines and names.
            return Err(validation(format!(
                "No aggregation found for path [{path}]"
            )));
        }
        let leaf = host
            .subs
            .iter()
            .find(|s| s.name == agg)
            .is_some_and(|s| metric_builder(s.metric).is_some());
        if let (true, Some(second)) = (leaf, second.as_ref()) {
            // ALLOC: one entry or message bounded by the request's pipelines and names.
            return Err(validation(format!(
                "No aggregation [{second}] found for path [{path}]"
            )));
        }
        // ALLOC: one entry or message bounded by the request's pipelines and names.
        return Err(DslError::unsupported(&format!(
            "the [buckets_path] [{path}] of the aggregation [{name}]"
        )));
    }
    if let BucketValue::Sub { name: sub, metric } = value {
        let _ = agg;
        if names.get(own).is_some_and(|n| n == sub) {
            // ALLOC: one entry or message bounded by the request's pipelines and names.
            return Err(validation(format!(
                "Cyclical dependency found with pipeline aggregator [{name}]"
            )));
        }
        check_sub(host, names, path, name, sub, metric)?;
    }
    Ok(())
}

fn check_sort(
    host: &MetricAgg,
    subs: &[String],
    names: &[String],
    name: &str,
    value: &BucketValue,
) -> Result<(), DslError> {
    if let BucketValue::Sub { name: sub, metric } = value {
        let path = if metric == "value" {
            sub.clone()
        } else {
            // ALLOC: one entry or message bounded by the request's pipelines and names.
            format!("{sub}.{metric}")
        };
        let _ = subs;
        check_sub(host, names, &path, name, sub, metric)?;
    }
    Ok(())
}

/// A path that names the sub-aggregation `sub` of `host` (or a sibling
/// parent pipeline) and reads `metric` of it.
fn check_sub(
    host: &MetricAgg,
    names: &[String],
    path: &str,
    name: &str,
    sub: &str,
    metric: &str,
) -> Result<(), DslError> {
    if names.iter().any(|n| n == sub) {
        return Ok(());
    }
    let Some(found) = host.subs.iter().find(|s| s.name == sub) else {
        // ALLOC: one entry or message bounded by the request's pipelines and names.
        return Err(validation(format!(
            "No aggregation found for path [{path}]"
        )));
    };
    if !reads_metric(found.metric, metric) {
        // ALLOC: one entry or message bounded by the request's pipelines and names.
        return Err(DslError::unsupported(&format!(
            "the [buckets_path] [{path}] of the aggregation [{name}]"
        )));
    }
    Ok(())
}

/// The refusal of a parent pipeline defined beside the aggregations rather
/// than inside a bucket aggregation: what validation says of the path it
/// reads, there being nothing to read it from.
pub fn at_top_level(pipe: &PipelineAgg) -> DslError {
    if pipe.kind.runs_script() {
        // ALLOC: error path; one message.
        return validation(format!(
            "{} aggregation [{}] must be declared inside of another aggregation",
            pipe.kind.name(),
            pipe.name
        ));
    }
    let path = match (&pipe.sort, pipe.kind) {
        (Some(sort), _) => match sort.fields.iter().find_map(|(v, _)| match v {
            BucketValue::Sub { name, metric } if metric == "value" => Some(name.clone()),
            // ALLOC: one entry or message bounded by the request's pipelines and names.
            BucketValue::Sub { name, metric } => Some(format!("{name}.{metric}")),
            // ALLOC: one entry or message bounded by the request's pipelines and names.
            BucketValue::Count => Some("_count".to_owned()),
            // ALLOC: one entry or message bounded by the request's pipelines and names.
            BucketValue::Key => Some("_key".to_owned()),
            BucketValue::Other { .. } => None,
        }) {
            Some(path) => path,
            // ALLOC: error path; one message.
            None => {
                return DslError::unsupported(
                    "a [bucket_sort] aggregation that sorts nothing, beside the aggregations",
                );
            }
        },
        (None, _) => pipe.path.clone(),
    };
    // ALLOC: error path; one message.
    validation(format!("No aggregation found for path [{path}]"))
}

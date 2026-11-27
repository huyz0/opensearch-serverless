//! The body of a sibling pipeline aggregation
//! (`BucketMetricsPipelineAggregationBuilder`'s parser) and the checks
//! `AggregatorFactories.Builder.validate` makes of its `buckets_path`.

use crate::aggs::metric::{Metric, MetricAgg};
use crate::aggs::pipeline::{BucketValue, PipelineAgg, PipelineKind};
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::failure::Failure;

/// Reads the body of a `kind` aggregation named `name`, the parser on its
/// start object.
pub fn parse_body(
    p: &mut Parser<'_>,
    name: &str,
    kind: PipelineKind,
) -> Result<PipelineAgg, DslError> {
    let mut path: Option<String> = None;
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
        match (key, token) {
            // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
            ("buckets_path", Kind::String) => path = Some(p.text().to_owned()),
            ("buckets_path", Kind::StartArray) => {
                // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
                let mut paths = Vec::new();
                loop {
                    match p.next_token() {
                        Some(Kind::EndArray) | None => break,
                        // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
                        Some(Kind::String) => paths.push(p.text().to_owned()),
                        Some(other) => return Err(unexpected(p, other, key, name)),
                    }
                }
                if paths.len() != 1 {
                    // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
                    return Err(DslError::unsupported(&format!(
                        "a [{}] aggregation with {} [buckets_path]s",
                        kind.name(),
                        paths.len()
                    )));
                }
                path = paths.pop();
            }
            ("gap_policy", Kind::String) => match p.text() {
                "skip" => insert_zeros = false,
                "insert_zeros" => insert_zeros = true,
                other => {
                    return Err(DslError::parsing(
                        p.location(),
                        // ALLOC: error path; one message.
                        format!(
                            "Invalid gap policy: [{other}], accepted values: [insert_zeros, skip]"
                        ),
                    ));
                }
            },
            ("format", Kind::String) => {
                // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
                return Err(DslError::unsupported(&format!(
                    "the [format] of a [{}] aggregation",
                    kind.name()
                )));
            }
            (_, Kind::StartObject | Kind::StartArray) => {
                return Err(unexpected(p, token, key, name));
            }
            (_, other) => return Err(unexpected(p, other, key, name)),
        }
    }
    let Some(path) = path else {
        return Err(DslError::parsing(
            p.location(),
            // ALLOC: error path; one message.
            format!("Missing required field [buckets_path] for aggregation [{name}]"),
        ));
    };
    let (agg, value) = split_path(&path, name, kind)?;
    Ok(PipelineAgg {
        // ALLOC: one name a pipeline aggregation, bounded by the body's recording.
        name: name.to_owned(),
        kind,
        path,
        agg,
        value,
        insert_zeros,
        host: None,
        lag: 1,
        sort: None,
    })
}

fn unexpected(p: &Parser<'_>, token: Kind, key: &str, name: &str) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!("Unexpected token {} [{key}] in [{name}]", token.name()),
    )
}

/// `AggregationPath.parse` of the two shapes served: `agg>_count`,
/// `agg>sub` and `agg>sub.metric`. Any other path parses as
/// [`BucketValue::Other`], which validation then refuses.
pub(super) fn split_path(
    path: &str,
    name: &str,
    kind: PipelineKind,
) -> Result<(String, BucketValue), DslError> {
    let mut elements = path.split('>');
    let agg = elements.next().unwrap_or_default();
    // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
    let rest: Vec<&str> = elements.collect();
    let refuse = || {
        // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
        DslError::unsupported(&format!(
            "the [buckets_path] [{path}] of the [{}] aggregation [{name}]",
            kind.name()
        ))
    };
    if agg.is_empty() || agg.contains(['[', '.']) {
        return Err(refuse());
    }
    // ALLOC: one name a pipeline aggregation, bounded by the body's recording.
    let agg = agg.to_owned();
    let [sub] = rest.as_slice() else {
        // ALLOC: as above.
        let second = rest.first().map(|s| (*s).to_owned());
        return Ok((agg, BucketValue::Other { second }));
    };
    if sub.is_empty() || sub.contains('[') {
        return Err(refuse());
    }
    let value = if *sub == "_count" {
        BucketValue::Count
    } else {
        let (sub, metric) = sub.split_once('.').unwrap_or((sub, "value"));
        if sub.is_empty() || metric.is_empty() || sub.starts_with('_') {
            return Err(refuse());
        }
        BucketValue::Sub {
            // ALLOC: one name a pipeline aggregation, bounded by the body's recording.
            name: sub.to_owned(),
            // ALLOC: as above.
            metric: metric.to_owned(),
        }
    };
    Ok((agg, value))
}

pub(super) fn validation(reason: String) -> DslError {
    DslError::Failure(Failure::plain(
        "action_request_validation_exception",
        // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
        format!("Validation Failed: 1: {reason};"),
    ))
}

/// `AggregatorFactories.Builder.validate` of `pipelines` against the
/// top-level `aggs`: the aggregation each path names must exist and hold
/// buckets, and hold the sub-aggregation it names.
///
/// # Errors
///
/// The request's validation failure, or a refusal of a path this node cannot
/// resolve.
pub fn validate(aggs: &[MetricAgg], pipelines: &[PipelineAgg]) -> Result<(), DslError> {
    // A parent pipeline was checked inside the aggregation that holds it.
    // ALLOC: one entry a pipeline aggregation, bounded by the request's.
    let siblings: Vec<&PipelineAgg> = pipelines.iter().filter(|p| p.host.is_none()).collect();
    for pipeline in siblings.iter().copied() {
        if aggs.iter().any(|a| a.name == pipeline.name)
            || siblings.iter().filter(|q| q.name == pipeline.name).count() > 1
        {
            return Err(DslError::illegal(
                // ALLOC: error path; one message.
                format!(
                    "Two sibling aggregations cannot have the same name: [{}]",
                    pipeline.name
                ),
            ));
        }
        let Some(target) = aggs.iter().find(|a| a.name == pipeline.agg) else {
            return Err(validation(if pipeline.agg == pipeline.name {
                // ALLOC: error path; one message.
                format!(
                    "Cyclical dependency found with pipeline aggregator [{}]",
                    pipeline.name
                )
            } else {
                // ALLOC: error path; one message.
                format!("No aggregation found for path [{}]", pipeline.path)
            }));
        };
        if !has_buckets(target.metric) {
            let Some(class) = metric_builder(target.metric) else {
                // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
                return Err(DslError::unsupported(&format!(
                    "a [{}] aggregation over a [{}] aggregation",
                    pipeline.kind.name(),
                    target.metric.name()
                )));
            };
            // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
            return Err(validation(format!(
                "The first aggregation in buckets_path must be a multi-bucket aggregation for \
                 aggregation [{}] found :org.opensearch.search.aggregations.metrics.{class} for \
                 buckets path: {}",
                pipeline.name, pipeline.path
            )));
        }
        if let BucketValue::Other { second } = &pipeline.value {
            if let Some(second) = second.as_ref().filter(|s| !s.is_empty() && *s != "_count")
                && !target
                    .subs
                    .iter()
                    .any(|s| s.name == second.split('.').next().unwrap_or(second))
            {
                // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
                return Err(validation(format!(
                    "No aggregation [{second}] found for path [{}]",
                    pipeline.path
                )));
            }
            // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
            return Err(DslError::unsupported(&format!(
                "the [buckets_path] [{}] of the [{}] aggregation [{}]",
                pipeline.path,
                pipeline.kind.name(),
                pipeline.name
            )));
        }
        if let BucketValue::Sub { name, metric } = &pipeline.value {
            let Some(sub) = target.subs.iter().find(|s| &s.name == name) else {
                // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
                return Err(validation(format!(
                    "No aggregation [{name}] found for path [{}]",
                    pipeline.path
                )));
            };
            if !reads_metric(sub.metric, metric) {
                // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
                return Err(DslError::unsupported(&format!(
                    "the [buckets_path] [{}] of the [{}] aggregation [{}]",
                    pipeline.path,
                    pipeline.kind.name(),
                    pipeline.name
                )));
            }
        }
    }
    Ok(())
}

/// Whether `metric` is a bucket aggregation whose rendered body this node
/// reads buckets from.
pub(super) fn has_buckets(metric: Metric) -> bool {
    matches!(
        metric,
        Metric::Terms
            | Metric::Histogram
            | Metric::DateHistogram
            | Metric::Range
            | Metric::DateRange
    )
}

/// The `…AggregationBuilder` class of a metric a path may wrongly start at.
pub(super) fn metric_builder(metric: Metric) -> Option<&'static str> {
    Some(match metric {
        Metric::Min => "MinAggregationBuilder",
        Metric::Max => "MaxAggregationBuilder",
        Metric::Sum => "SumAggregationBuilder",
        Metric::Avg => "AvgAggregationBuilder",
        Metric::Stats => "StatsAggregationBuilder",
        Metric::ValueCount => "ValueCountAggregationBuilder",
        Metric::Cardinality => "CardinalityAggregationBuilder",
        _ => return None,
    })
}

/// Whether the sub-aggregation of type `sub` has the member `metric` in its
/// body as a number: `value` of the single-value metrics, and a stats'
/// `count`, `min`, `max`, `avg` and `sum`.
pub(super) fn reads_metric(sub: Metric, metric: &str) -> bool {
    match sub {
        Metric::Min
        | Metric::Max
        | Metric::Sum
        | Metric::Avg
        | Metric::ValueCount
        | Metric::Cardinality => metric == "value",
        Metric::Stats => matches!(metric, "count" | "min" | "max" | "avg" | "sum"),
        _ => false,
    }
}

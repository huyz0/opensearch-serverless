//! A search body's `aggs`: `AggregatorFactories.parseAggregators` and the
//! `ObjectParser` of each served metric's builder
//! (`ValuesSourceAggregationBuilder.declareFields`), at OpenSearch 3.10.0.
//!
//! Every aggregation type OpenSearch registers but this node does not serve
//! is refused as not supported yet; a type it does not know fails as it
//! fails there ("Unknown aggregation type [..]"). Of a metric's keys, only
//! `field` is served.

use crate::aggs::metric::{Metric, MetricAgg};
use crate::dsl::error::{DslError, parsing_failure};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::suggest;
use crate::failure::Failure;

/// Every aggregation type the OpenSearch 3.10.0 distribution registers,
/// pipelines included (`SearchModule.registerAggregations` and
/// `registerPipelineAggregations`, and the modules' `getAggregations`):
/// the candidates of "did you mean".
pub const REGISTERED_AGGREGATIONS: &[&str] = &[
    "adjacency_matrix",
    "auto_date_histogram",
    "avg",
    "avg_bucket",
    "bucket_script",
    "bucket_selector",
    "bucket_sort",
    "cardinality",
    "children",
    "composite",
    "cumulative_sum",
    "date_histogram",
    "date_range",
    "derivative",
    "diversified_sampler",
    "extended_stats",
    "extended_stats_bucket",
    "filter",
    "filters",
    "geo_bounds",
    "geo_centroid",
    "geo_distance",
    "geohash_grid",
    "geotile_grid",
    "global",
    "histogram",
    "ip_range",
    "matrix_stats",
    "max",
    "max_bucket",
    "median_absolute_deviation",
    "min",
    "min_bucket",
    "missing",
    "moving_avg",
    "moving_fn",
    "multi_terms",
    "multivalue_doc_count",
    "nested",
    "parent",
    "percentile_ranks",
    "percentiles",
    "percentiles_bucket",
    "range",
    "rare_terms",
    "reverse_nested",
    "sampler",
    "scripted_metric",
    "serial_diff",
    "significant_terms",
    "significant_text",
    "stats",
    "stats_bucket",
    "sum",
    "sum_bucket",
    "terms",
    "top_hits",
    "value_count",
    "variable_width_histogram",
    "weighted_avg",
];

/// `parseAggregators(parser, 0)` with the parser on the `aggs` object's
/// start: the aggregations in the order given, none for an empty object.
pub fn parse_aggregations(p: &mut Parser<'_>) -> Result<Vec<MetricAgg>, DslError> {
    parse_level(p, 0)
}

/// What one aggregation's definition held.
struct Definition {
    agg: Option<MetricAgg>,
    has_subs: bool,
}

fn parse_level(p: &mut Parser<'_>, level: usize) -> Result<Vec<MetricAgg>, DslError> {
    // ALLOC: one entry an aggregation, bounded by the body's recording.
    let mut aggs: Vec<MetricAgg> = Vec::new();
    loop {
        let token = p.next_token();
        match token {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {}
            Some(other) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!(
                        "Unexpected token {} in [aggs]: aggregations definitions must start \
                         with the name of the aggregation.",
                        other.name()
                    ),
                ));
            }
        }
        let name = p.text();
        if name.is_empty() || name.contains(['[', ']', '>']) {
            return Err(DslError::parsing(
                p.location(),
                // ALLOC: error path; one message.
                format!(
                    "Invalid aggregation name [{name}]. Aggregation names can contain any \
                     character except '[', ']', and '>'"
                ),
            ));
        }
        let token = p.next_token();
        if token != Some(Kind::StartObject) {
            return Err(DslError::parsing(
                p.location(),
                // ALLOC: error path; one message.
                format!(
                    "Aggregation definition for [{name} starts with a [{}], expected a \
                     [START_OBJECT].",
                    token.map_or("null", Kind::name)
                ),
            ));
        }
        let definition = parse_definition(p, name, level)?;
        let Some(agg) = definition.agg else {
            return Err(DslError::parsing(
                p.location(),
                // ALLOC: error path; one message.
                format!("Missing definition for aggregation [{name}]"),
            ));
        };
        if definition.has_subs {
            return Err(DslError::Failure(Failure::opensearch(
                "aggregation_initialization_exception",
                // ALLOC: error path; one message.
                format!(
                    "Aggregator [{name}] of type [{}] cannot accept sub-aggregations",
                    agg.metric.name()
                ),
            )));
        }
        if aggs.iter().any(|a| a.name == agg.name) {
            return Err(DslError::illegal(
                // ALLOC: error path; one message.
                format!("Two sibling aggregations cannot have the same name: [{name}]"),
            ));
        }
        aggs.push(agg);
    }
    Ok(aggs)
}

/// One aggregation's object, the parser on its start.
fn parse_definition(p: &mut Parser<'_>, name: &str, level: usize) -> Result<Definition, DslError> {
    let mut definition = Definition {
        agg: None,
        has_subs: false,
    };
    let mut subs_seen = false;
    loop {
        let token = p.next_token();
        match token {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {}
            Some(other) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!(
                        "Expected [FIELD_NAME] under a [START_OBJECT], but got a [{}] in [{name}]",
                        other.name()
                    ),
                ));
            }
        }
        let key = p.text();
        let token = p.next_token();
        if token != Some(Kind::StartObject) {
            return Err(DslError::parsing(
                p.location(),
                // ALLOC: error path; one message.
                format!(
                    "Expected [START_OBJECT] under [{key}], but got a [{}] in [{name}]",
                    token.map_or("null", Kind::name)
                ),
            ));
        }
        match key {
            "meta" => return Err(DslError::unsupported("an aggregation's [meta]")),
            "aggregations" | "aggs" => {
                if subs_seen {
                    return Err(DslError::parsing(
                        p.location(),
                        // ALLOC: error path; one message.
                        format!("Found two sub aggregation definitions under [{name}]"),
                    ));
                }
                subs_seen = true;
                // `parseAggregators` gives no factories for an empty object.
                definition.has_subs = !parse_level(p, level.saturating_add(1))?.is_empty();
            }
            ty => {
                if let Some(agg) = &definition.agg {
                    return Err(DslError::parsing(
                        p.location(),
                        // ALLOC: error path; one message.
                        format!(
                            "Found two aggregation type definitions in [{name}]: [{}] and [{ty}]",
                            agg.metric.name()
                        ),
                    ));
                }
                let Some(metric) = Metric::from_name(ty) else {
                    if REGISTERED_AGGREGATIONS.contains(&ty) {
                        // ALLOC: error path; one message.
                        return Err(DslError::unsupported(&format!("the [{ty}] aggregation")));
                    }
                    // `NamedObjectNotFoundException`, the cause, is located
                    // where the definition's object starts.
                    let at = p.location();
                    let cause = Failure::plain(
                        "named_object_not_found_exception",
                        // ALLOC: error path; one message.
                        format!("[{}:{}] unknown field [{ty}]", at.line, at.column),
                    );
                    return Err(DslError::Failure(
                        parsing_failure(
                            at,
                            // ALLOC: error path; one message.
                            format!(
                                "Unknown aggregation type [{ty}]{}",
                                suggest(ty, REGISTERED_AGGREGATIONS)
                            ),
                        )
                        .caused_by(cause),
                    ));
                };
                let field = parse_metric_body(p, metric)?;
                definition.agg = Some(MetricAgg {
                    // ALLOC: one name an aggregation, bounded by the body's recording.
                    name: name.to_owned(),
                    metric,
                    field,
                });
            }
        }
    }
    Ok(definition)
}

/// The keys every metric here declares (`declareFields(PARSER, true, true,
/// false)`: scriptable and formattable).
const VALUES_SOURCE_KEYS: &[&str] = &["field", "missing", "value_type", "format", "script"];

/// The `ObjectParser` of `metric`'s builder over its object, the parser on
/// its start: the field it names.
fn parse_metric_body(p: &mut Parser<'_>, metric: Metric) -> Result<String, DslError> {
    let ty = metric.name();
    let mut field: Option<String> = None;
    let mut key = "";
    let mut key_at = p.location();
    loop {
        let token = p.next_token();
        match token {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                key = p.text();
                key_at = p.location();
                continue;
            }
            Some(kind) => {
                if !VALUES_SOURCE_KEYS.contains(&key) {
                    return Err(DslError::xcontent(
                        key_at,
                        // ALLOC: error path; one message.
                        &format!(
                            "[{ty}] unknown field [{key}]{}",
                            suggest(key, VALUES_SOURCE_KEYS)
                        ),
                        None,
                    ));
                }
                if key != "field" {
                    // ALLOC: error path; one message.
                    return Err(DslError::unsupported(&format!(
                        "the [{ty}] aggregation's [{key}]"
                    )));
                }
                if kind != Kind::String {
                    return Err(DslError::xcontent(
                        p.location(),
                        // ALLOC: error path; one message.
                        &format!(
                            "[{ty}] field doesn't support values of type: {}",
                            kind.name()
                        ),
                        None,
                    ));
                }
                // ALLOC: one field name an aggregation, bounded by the body's recording.
                field = Some(p.text().to_owned());
            }
        }
    }
    field.ok_or_else(|| {
        DslError::illegal("Required one of fields [field, script], but none were specified. ")
    })
}

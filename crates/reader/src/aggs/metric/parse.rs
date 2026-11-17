//! A search body's `aggs`: `AggregatorFactories.parseAggregators` and the
//! `ObjectParser` of each served metric's builder
//! (`ValuesSourceAggregationBuilder.declareFields`), at OpenSearch 3.10.0.
//!
//! Every aggregation type OpenSearch registers but this node does not serve
//! is refused as not supported yet; a type it does not know fails as it
//! fails there ("Unknown aggregation type [..]"). Of a metric's keys,
//! `field` and each metric's own options are served
//! ([`parse_percentiles_body`] for the two percentile metrics).

use crate::aggs::metric::percentiles::parse_percentiles_body;
use crate::aggs::metric::{CardinalityOptions, Metric, MetricAgg, PercentilesOptions};
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
                let (field, cardinality, percentiles) = match metric {
                    Metric::Percentiles | Metric::PercentileRanks => {
                        let (field, options) = parse_percentiles_body(p, metric, name)?;
                        (field, CardinalityOptions::default(), options)
                    }
                    _ => {
                        let (field, options) = parse_metric_body(p, metric, name)?;
                        (field, options, PercentilesOptions::default())
                    }
                };
                definition.agg = Some(MetricAgg {
                    // ALLOC: one name an aggregation, bounded by the body's recording.
                    name: name.to_owned(),
                    metric,
                    field,
                    cardinality,
                    percentiles,
                });
            }
        }
    }
    Ok(definition)
}

/// The keys every metric here but `cardinality` declares
/// (`declareFields(PARSER, true, true, false)`: scriptable and formattable).
const VALUES_SOURCE_KEYS: &[&str] = &["field", "missing", "value_type", "format", "script"];

/// `CardinalityAggregationBuilder`'s keys: `declareFields(PARSER, true,
/// false, false)`, not formattable, and its own three.
const CARDINALITY_KEYS: &[&str] = &[
    "field",
    "missing",
    "value_type",
    "script",
    "precision_threshold",
    "execution_hint",
    "rehash",
];

/// The `ObjectParser` of `metric`'s builder over its object, the parser on
/// its start: the field it names, and a `cardinality`'s options.
fn parse_metric_body(
    p: &mut Parser<'_>,
    metric: Metric,
    name: &str,
) -> Result<(String, CardinalityOptions), DslError> {
    let ty = metric.name();
    let keys = if metric == Metric::Cardinality {
        CARDINALITY_KEYS
    } else {
        VALUES_SOURCE_KEYS
    };
    let mut field: Option<String> = None;
    let mut options = CardinalityOptions::default();
    let mut key = "";
    let mut key_at = p.location();
    loop {
        let token = p.next_token();
        let kind = match token {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                key = p.text();
                key_at = p.location();
                continue;
            }
            Some(kind) => kind,
        };
        if !keys.contains(&key) {
            return Err(DslError::xcontent(
                key_at,
                // ALLOC: error path; one message.
                &format!("[{ty}] unknown field [{key}]{}", suggest(key, keys)),
                None,
            ));
        }
        let accepted = match key {
            "field" | "execution_hint" => kind == Kind::String,
            "precision_threshold" | "rehash" => matches!(kind, Kind::Number | Kind::String),
            _ => {
                // ALLOC: error path; one message.
                return Err(DslError::unsupported(&format!(
                    "the [{ty}] aggregation's [{key}]"
                )));
            }
        };
        if !accepted {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[{ty}] {key} doesn't support values of type: {}",
                    kind.name()
                ),
                None,
            ));
        }
        match key {
            // ALLOC: one field name an aggregation, bounded by the body's recording.
            "field" => field = Some(p.text().to_owned()),
            // ALLOC: one hint an aggregation, bounded by the body's recording.
            "execution_hint" => options.execution_hint = Some(p.text().to_owned()),
            _ => {
                let failed = |cause: DslError| {
                    DslError::xcontent(
                        p.location(),
                        // ALLOC: error path; one message.
                        &format!("[{ty}] failed to parse field [{key}]"),
                        Some(cause),
                    )
                };
                let value = long_value(kind, p.text()).map_err(failed)?;
                if key == "precision_threshold" {
                    if value < 0 {
                        return Err(failed(DslError::illegal(
                            // ALLOC: error path; one message.
                            format!(
                                "[precisionThreshold] must be greater than or equal to 0. \
                                 Found [{value}] in [{name}]"
                            ),
                        )));
                    }
                    options.precision_threshold = Some(value);
                }
            }
        }
    }
    let field = field.ok_or_else(|| {
        DslError::illegal("Required one of fields [field, script], but none were specified. ")
    })?;
    Ok((field, options))
}

/// `parser.longValue()` with coercion, the parser on a number or a string:
/// a number as Jackson reads it (a fraction truncated), a string as
/// `Numbers.toLong(text, true)` reads it.
fn long_value(kind: Kind, text: &str) -> Result<i64, DslError> {
    if let Ok(n) = text.parse::<i64>() {
        return Ok(n);
    }
    // 2^63, exact as a double.
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    let decimal = is_big_decimal(text);
    let value = if decimal {
        text.parse::<f64>().ok()
    } else {
        None
    };
    if kind == Kind::Number {
        return match value {
            // ARITH: none; `as` truncates, and the value is inside a long.
            #[allow(clippy::cast_possible_truncation)]
            Some(v) if text.contains(['.', 'e', 'E']) && (-LIMIT..=LIMIT).contains(&v) => {
                Ok(v as i64)
            }
            _ => Err(DslError::Failure(Failure::plain(
                "input_coercion_exception",
                // ALLOC: error path; one message.
                format!(
                    "Numeric value ({text}) out of range of `long` (-9223372036854775808 - \
                     9223372036854775807)"
                ),
            ))),
        };
    }
    match value {
        // ARITH: none; `as` truncates toward zero as `toBigInteger` does,
        // and the value is inside a long.
        #[allow(clippy::cast_possible_truncation)]
        Some(v) if v > -LIMIT - 1.0 && v < LIMIT => Ok(v as i64),
        Some(_) => Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!("Value [{text}] is out of range for a long"),
        )),
        // ALLOC: error path; one message.
        None => Err(DslError::illegal(format!("For input string: \"{text}\""))),
    }
}

/// Whether `new BigDecimal(text)` reads `text`: a sign, digits with at most
/// one point and at least one digit, and an exponent.
fn is_big_decimal(text: &str) -> bool {
    let body = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(at) => (
            body.get(..at).unwrap_or_default(),
            body.get(at.saturating_add(1)..),
        ),
        None => (body, None),
    };
    let mut parts = mantissa.splitn(2, '.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next().unwrap_or_default();
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok =
        digits(whole) && digits(fraction) && !(whole.is_empty() && fraction.is_empty());
    let exponent_ok = exponent.is_none_or(|e| {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && digits(e)
    });
    mantissa_ok && exponent_ok
}

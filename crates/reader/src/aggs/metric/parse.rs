//! A search body's `aggs`: `AggregatorFactories.parseAggregators` and the
//! `ObjectParser` of each served metric's builder
//! (`ValuesSourceAggregationBuilder.declareFields`), at OpenSearch 3.10.0.
//!
//! Every aggregation type OpenSearch registers but this node does not serve
//! is refused as not supported yet; a type it does not know fails as it
//! fails there ("Unknown aggregation type [..]"). Of a metric's keys,
//! `field` and each metric's own options are served
//! ([`parse_percentiles_body`] for the two percentile metrics).

use crate::aggs::bucket::parse_arms::parse_bucket;
pub(crate) use crate::aggs::metric::long_value::long_value;
use crate::aggs::metric::percentiles::parse_percentiles_body;
pub use crate::aggs::metric::registered::REGISTERED_AGGREGATIONS;
use crate::aggs::metric::sub_rules::{holds_subs, is_scalar, nests_under};
use crate::aggs::metric::top_hits::parse_top_hits_body;
use crate::aggs::metric::{CardinalityOptions, Metric, MetricAgg, PercentilesOptions};
use crate::aggs::pipeline::parent_parse;
use crate::aggs::pipeline::parse as pipeline_parse;
use crate::aggs::pipeline::{PipelineAgg, PipelineKind};
use crate::dsl::error::{DslError, parsing_failure};
use crate::dsl::queries::function_score_script::ScriptScore;
use crate::dsl::tokens::{Kind, Parser};

use super::{geo_parse, script_values};
use crate::aggs::metric::scripted_metric;
use crate::dsl::values::suggest;
use crate::failure::Failure;

/// `parseAggregators(parser, 0)` with the parser on the `aggs` object's
/// start: the aggregations in the order given, none for an empty object.
pub fn parse_aggregations(p: &mut Parser<'_>) -> Result<Vec<MetricAgg>, DslError> {
    parse_aggregations_with_pipelines(p).map(|(aggs, _)| aggs)
}

/// [`parse_aggregations`], and the top-level sibling pipeline aggregations,
/// checked against the aggregations beside them.
pub fn parse_aggregations_with_pipelines(
    p: &mut Parser<'_>,
) -> Result<(Vec<MetricAgg>, Vec<PipelineAgg>), DslError> {
    // ALLOC: one entry a pipeline aggregation, bounded by the body's recording.
    let mut pipes = Vec::new();
    let aggs = parse_level(p, 0, &mut pipes)?;
    pipeline_parse::validate(&aggs, &pipes)?;
    Ok((aggs, pipes))
}

/// The deepest an aggregation holding sub-aggregations may sit (the top level
/// is 0): four levels of aggregations.
const MAX_NESTING: usize = 3;

/// What one aggregation's definition held.
struct Definition {
    agg: Option<MetricAgg>,
    subs: Vec<MetricAgg>,
    /// The definition was a sibling pipeline aggregation, pushed to the sink.
    piped: bool,
    /// The parent pipeline aggregations among its `aggs`.
    parents: Vec<PipelineAgg>,
}

fn parse_level(
    p: &mut Parser<'_>,
    level: usize,
    pipes: &mut Vec<PipelineAgg>,
) -> Result<Vec<MetricAgg>, DslError> {
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
        let definition = parse_definition(p, name, level, pipes)?;
        if definition.piped {
            continue;
        }
        let Some(agg) = definition.agg else {
            return Err(DslError::parsing(
                p.location(),
                // ALLOC: error path; one message.
                format!("Missing definition for aggregation [{name}]"),
            ));
        };
        let mut agg = agg;
        if !definition.subs.is_empty() {
            if !holds_subs(agg.metric) {
                return Err(sub_refusal(&agg, name));
            }
            if agg.metric == Metric::Composite
                && let Some(sub) = definition.subs.iter().find(|s| !is_scalar(s.metric))
            {
                // ALLOC: error path; one message.
                return Err(DslError::unsupported(&format!(
                    "a [{}] sub-aggregation of a [composite] aggregation",
                    sub.metric.name()
                )));
            }
            if matches!(agg.metric, Metric::Nested | Metric::ReverseNested)
                && let Some(sub) = scored_sub(&definition.subs)
            {
                // The engine reads no scores under a nested join.
                // ALLOC: error path; one message.
                return Err(DslError::unsupported(&format!(
                    "a [{}] sub-aggregation of a [{}] aggregation",
                    sub.metric.name(),
                    agg.metric.name()
                )));
            }
            if let Some(sub) = definition.subs.iter().find(|s| {
                !is_scalar(s.metric)
                    && s.metric != Metric::Terms
                    && s.metric != Metric::TopHits
                    && !nests_under(s.metric, agg.metric)
            }) {
                // ALLOC: error path; one message.
                return Err(DslError::unsupported(&format!(
                    "a [{}] sub-aggregation of a [{}] aggregation",
                    sub.metric.name(),
                    agg.metric.name()
                )));
            }
            if level >= MAX_NESTING {
                // ALLOC: error path; one message.
                return Err(DslError::unsupported(&format!(
                    "aggregations nested more than {MAX_NESTING} levels under [{name}]"
                )));
            }
            agg.subs = definition.subs;
        }
        if !definition.parents.is_empty() {
            if !holds_subs(agg.metric) {
                return Err(sub_refusal(&agg, name));
            }
            let mut parents = definition.parents;
            parent_parse::validate_parents(&agg, &mut parents)?;
            pipes.extend(parents);
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

/// The first `top_hits` among `subs` or beneath them: the engine reads no
/// scores under a nested join.
fn scored_sub(subs: &[MetricAgg]) -> Option<&MetricAgg> {
    subs.iter().find_map(|s| {
        if s.metric == Metric::TopHits {
            Some(s)
        } else {
            scored_sub(&s.subs)
        }
    })
}

/// The refusal of sub-aggregations under `agg`, which holds none here.
fn sub_refusal(agg: &MetricAgg, name: &str) -> DslError {
    DslError::Failure(Failure::opensearch(
        "aggregation_initialization_exception",
        // ALLOC: error path; one message.
        format!(
            "Aggregator [{name}] of type [{}] cannot accept sub-aggregations",
            agg.metric.name()
        ),
    ))
}

/// One aggregation's object, the parser on its start.
fn parse_definition(
    p: &mut Parser<'_>,
    name: &str,
    level: usize,
    pipes: &mut Vec<PipelineAgg>,
) -> Result<Definition, DslError> {
    let mut definition = Definition {
        agg: None,
        // ALLOC: empty; the definition's `aggs` fill it.
        subs: Vec::new(),
        piped: false,
        // ALLOC: empty; the definition's `aggs` fill it.
        parents: Vec::new(),
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
                definition.subs = parse_level(p, level.saturating_add(1), &mut definition.parents)?;
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
                if let Some(kind) = PipelineKind::served(ty) {
                    // A parent pipeline sits in a top-level aggregation (level 1).
                    if level > 0 && !(kind.is_parent() && level == 1) {
                        // ALLOC: error path; one message.
                        return Err(DslError::unsupported(&format!(
                            "a [{ty}] aggregation under another aggregation"
                        )));
                    }
                    if kind.is_parent() {
                        let pipe = parent_parse::parse_body(p, name, kind)?;
                        if level == 0 {
                            return Err(parent_parse::at_top_level(&pipe));
                        }
                        pipes.push(pipe);
                    } else {
                        pipes.push(pipeline_parse::parse_body(p, name, kind)?);
                    }
                    definition.piped = true;
                    continue;
                }
                let named = Metric::from_name(ty).filter(|m| {
                    *m != Metric::ScriptedMetric || crate::painless_switch::native_compile()
                });
                let Some(metric) = named else {
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
                let mut agg = MetricAgg {
                    // ALLOC: one name an aggregation, bounded by the body's recording.
                    name: name.to_owned(),
                    metric,
                    // ALLOC: empty; filled by the parse of its body.
                    field: String::new(),
                    cardinality: CardinalityOptions::default(),
                    percentiles: PercentilesOptions::default(),
                    sigma: crate::aggs::metric::DEFAULT_SIGMA,
                    top_hits: None,
                    terms: None,
                    histogram: None,
                    date_histogram: None,
                    auto_date: None,
                    range: None,
                    filters: None,
                    nested: None,
                    composite: None,
                    significant: None,
                    rare: None,
                    scripted: None,
                    script: None,
                    wrap_longitude: true,
                    // ALLOC: empty; the definition's `aggs` fill it.
                    subs: Vec::new(),
                };
                if !parse_bucket(p, &mut agg)? {
                    match metric {
                        Metric::Percentiles | Metric::PercentileRanks => {
                            let (field, options) = parse_percentiles_body(p, metric, name)?;
                            (agg.field, agg.percentiles) = (field, options);
                        }
                        Metric::TopHits => {
                            // ALLOC: one box a `top_hits`, bounded by the body's recording.
                            agg.top_hits = Some(Box::new(parse_top_hits_body(p, name)?));
                        }
                        Metric::GeoCentroid | Metric::GeoBounds => {
                            (agg.field, agg.wrap_longitude) = geo_parse::parse_body(p, metric)?;
                        }
                        Metric::ScriptedMetric => {
                            // ALLOC: one box a `scripted_metric`, bounded by the body's recording.
                            agg.scripted = Some(Box::new(scripted_metric::parse_body(p, name)?));
                        }
                        _ => {
                            let (field, options, script, sigma) =
                                parse_metric_body(p, metric, name)?;
                            (agg.field, agg.cardinality, agg.script, agg.sigma) =
                                (field, options, script, sigma);
                        }
                    }
                }
                definition.agg = Some(agg);
            }
        }
    }
    Ok(definition)
}

/// A metric body's field, `cardinality` options, script and `sigma`.
type MetricBody = (String, CardinalityOptions, Option<Box<ScriptScore>>, f64);

/// The `ObjectParser` of `metric`'s builder over its object, the parser on
/// its start: the field it names, and a `cardinality`'s options.
fn parse_metric_body(
    p: &mut Parser<'_>,
    metric: Metric,
    name: &str,
) -> Result<MetricBody, DslError> {
    let ty = metric.name();
    let mut sigma = crate::aggs::metric::DEFAULT_SIGMA;
    let keys = if metric == Metric::Cardinality {
        super::body_keys::CARDINALITY_KEYS
    } else if metric == Metric::ExtendedStats {
        super::body_keys::KEYS
    } else {
        super::body_keys::VALUES_SOURCE_KEYS
    };
    let mut field: Option<String> = None;
    let mut script: Option<Box<ScriptScore>> = None;
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
        if key == "script" && script_values::takes_script(metric) {
            // ALLOC: one box a script, bounded by the source's size.
            script = Some(Box::new(script_values::parse(p, kind)?));
            continue;
        }
        let accepted = match key {
            "field" | "execution_hint" => kind == Kind::String,
            "precision_threshold" | "rehash" | "sigma" => {
                matches!(kind, Kind::Number | Kind::String)
            }
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
            "sigma" => sigma = super::body_keys::sigma(p, kind, name)?,
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
    if script.is_some() {
        if field.is_some() {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!(
                "the [{ty}] aggregation's [script] with a [field]"
            )));
        }
        // ALLOC: empty; a scripted aggregation reads no field.
        return Ok((String::new(), options, script, sigma));
    }
    let field = field.ok_or_else(|| {
        DslError::illegal("Required one of fields [field, script], but none were specified. ")
    })?;
    Ok((field, options, None, sigma))
}

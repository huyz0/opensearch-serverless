//! Search pipelines: what a stored definition may hold, and what its
//! processors do to a search.
//!
//! Ported from the common module's `OversampleRequestProcessor`,
//! `TruncateHitsResponseProcessor` and `FilterQueryRequestProcessor`, and from
//! the shell's `ServerlessSearchPipelines` for what is refused. Core's engine
//! is not ported: this runs exactly the processors below, and a definition
//! naming any other is refused when it is stored, so a stored pipeline never
//! silently ignores part of itself.
//!
//! A response processor's effect here is decided from the definition and the
//! request alone (`truncate_hits` cuts to a number the request side already
//! fixed), so [`SearchPipeline::apply`] folds it into
//! [`SearchSource::response_size_cap`]: the search runs with the size the
//! request processors left, and the hits answered are cut to the cap.

use std::collections::HashMap;

use oss_doc::mapping::json::{Json, parse};
use oss_rest::json::{Limits, Tokenizer};

use crate::dsl::{BoolQuery, Query, Recorder, SearchSource};

/// The request processors this node builds.
pub const REQUEST_PROCESSORS: &[&str] = &["filter_query", "oversample"];

/// The response processors this node builds.
pub const RESPONSE_PROCESSORS: &[&str] = &["truncate_hits"];

/// Processors OpenSearch ships (the common module) that this node does not
/// build: refused by name, not as unknown.
const NOT_BUILT: &[&str] = &[
    "script",
    "acl_routing_search",
    "hierarchical_routing_search",
    "temporal_routing_search",
    "rename_field",
    "collapse",
    "sort",
    "split",
];

const TOP_KEYS: &[&str] = &[
    "description",
    "version",
    "request_processors",
    "response_processors",
    "phase_results_processors",
];

/// The options every processor takes.
const COMMON: &[&str] = &["tag", "description", "ignore_failure"];

/// The context attribute `oversample` sets and `truncate_hits` reads.
const ORIGINAL_SIZE: &str = "original_size";

/// Why a definition was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The definition is wrong: 400 `invalid_pipeline`.
    Invalid(String),
    /// The definition is valid OpenSearch this node does not run: 501.
    Unsupported(String),
}

/// A processor failed while running over a search: 500.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunError(String);

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RunError {}

#[derive(Debug, Clone)]
enum RequestStep {
    Oversample { factor: f64, prefix: String },
    Filter(Query),
}

#[derive(Debug, Clone)]
enum ResponseStep {
    Truncate {
        target: Option<usize>,
        prefix: String,
    },
}

/// A compiled definition.
#[derive(Debug, Clone)]
pub struct SearchPipeline {
    request: Vec<RequestStep>,
    response: Vec<ResponseStep>,
}

fn supported() -> String {
    // ALLOC: error path; one message.
    format!(
        "supported request processors: {}; response processors: {}",
        REQUEST_PROCESSORS.join(", "),
        RESPONSE_PROCESSORS.join(", ")
    )
}

fn invalid(reason: impl Into<String>) -> Refused {
    Refused::Invalid(reason.into())
}

fn unsupported(what: &str) -> Refused {
    // ALLOC: error path; one message.
    Refused::Unsupported(format!(
        "{what} is not supported by this node; {}",
        supported()
    ))
}

impl SearchPipeline {
    /// Compiles a stored definition.
    ///
    /// # Errors
    ///
    /// [`Refused::Invalid`] for a body that is not a definition, an unknown
    /// processor or option, or a bad value; [`Refused::Unsupported`] for a
    /// processor OpenSearch has and this node does not build, for
    /// `phase_results_processors`, and for `ignore_failure: true`.
    pub fn compile(body: &[u8]) -> Result<Self, Refused> {
        let Ok(Json::Obj(top)) = parse(body) else {
            return Err(invalid(
                "could not parse the pipeline: the body is not a JSON object",
            ));
        };
        if let Some((k, _)) = top.iter().find(|(k, _)| !TOP_KEYS.contains(&k.as_str())) {
            // ALLOC: error path; one message.
            return Err(invalid(format!("unknown pipeline field [{k}]")));
        }
        let def = Json::Obj(top);
        if let Some(phase) = def.get("phase_results_processors") {
            match phase {
                Json::Arr(items) if items.is_empty() => {}
                Json::Arr(_) => return Err(unsupported("[phase_results_processors]")),
                _ => return Err(invalid("[phase_results_processors] must be an array")),
            }
        }
        let request = list(def.get("request_processors"), true)?
            .into_iter()
            .map(|(kind, config)| request_step(kind, config))
            .collect::<Result<_, _>>()?;
        let response = list(def.get("response_processors"), false)?
            .into_iter()
            .map(|(kind, config)| response_step(kind, config))
            .collect::<Result<_, _>>()?;
        Ok(Self { request, response })
    }

    /// Runs the request processors over `source` and fixes what the response
    /// processors will cut to.
    ///
    /// # Errors
    ///
    /// [`RunError`] for a `truncate_hits` with no `target_size` and no
    /// earlier `oversample` under its prefix.
    pub fn apply(&self, source: &mut SearchSource) -> Result<(), RunError> {
        // ALLOC: one entry an oversample processor.
        let mut context: HashMap<String, usize> = HashMap::new();
        for step in &self.request {
            match step {
                RequestStep::Oversample { factor, prefix } => {
                    let original = source.size;
                    // ALLOC: one key a processor.
                    context.insert(key(prefix), original);
                    source.size = oversized(original, *factor);
                }
                RequestStep::Filter(filter) => {
                    let mut wrapped = BoolQuery {
                        adjust_pure_negative: true,
                        boost: 1.0,
                        ..BoolQuery::default()
                    };
                    wrapped.filter.push(filter.clone());
                    if let Some(old) = source.query.take() {
                        wrapped.must.push(old);
                    }
                    // ALLOC: compiled once a search that names the pipeline; bounded by its definition (at most 1 MiB).
                    source.query = Some(Query::Bool(Box::new(wrapped)));
                }
            }
        }
        for step in &self.response {
            let ResponseStep::Truncate { target, prefix } = step;
            let size = match target {
                Some(n) => *n,
                None => *context.get(&key(prefix)).ok_or_else(|| {
                    // ALLOC: error path; one message.
                    RunError(format!(
                        "Must specify target_size unless an earlier processor set {}",
                        key(prefix)
                    ))
                })?,
            };
            source.response_size_cap = Some(source.response_size_cap.map_or(size, |c| c.min(size)));
        }
        Ok(())
    }
}

/// `ContextUtils.applyContextPrefix` for the original size.
fn key(prefix: &str) -> String {
    if prefix.is_empty() {
        // ALLOC: compiled once a search that names the pipeline; bounded by its definition (at most 1 MiB).
        ORIGINAL_SIZE.to_owned()
    } else {
        // ALLOC: one key a processor.
        format!("{prefix}.{ORIGINAL_SIZE}")
    }
}

/// `(int) Math.ceil(size * factor)`, kept inside what a window can hold.
fn oversized(size: usize, factor: f64) -> usize {
    #[allow(clippy::cast_precision_loss)]
    let grown = (size as f64 * factor).ceil();
    if grown >= f64::from(u32::MAX) {
        return u32::MAX as usize;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        grown as usize
    }
}

/// The one-key objects of a processor list, as (type, options).
fn list(value: Option<&Json>, request: bool) -> Result<Vec<(&str, &Json)>, Refused> {
    let (field, own, other) = if request {
        (
            "request_processors",
            REQUEST_PROCESSORS,
            RESPONSE_PROCESSORS,
        )
    } else {
        (
            "response_processors",
            RESPONSE_PROCESSORS,
            REQUEST_PROCESSORS,
        )
    };
    let Some(value) = value else {
        // ALLOC: compiled once a search that names the pipeline; bounded by its definition (at most 1 MiB).
        return Ok(Vec::new());
    };
    let Json::Arr(items) = value else {
        // ALLOC: error path; one message.
        return Err(invalid(format!("[{field}] must be an array of processors")));
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Json::Obj(members) = item else {
            return Err(invalid("a processor must be an object of one type"));
        };
        let [(kind, config)] = members.as_slice() else {
            return Err(invalid("a processor must be an object of one type"));
        };
        if NOT_BUILT.contains(&kind.as_str()) {
            // ALLOC: error path; one message.
            return Err(unsupported(&format!("search processor [{kind}]")));
        }
        if !own.contains(&kind.as_str()) {
            // ALLOC: error path; one message.
            let hint = if other.contains(&kind.as_str()) {
                format!(" ([{kind}] is a processor of the other kind)")
            } else {
                // ALLOC: compiled once a search that names the pipeline; bounded by its definition (at most 1 MiB).
                String::new()
            };
            // ALLOC: compiled once a search that names the pipeline; bounded by its definition (at most 1 MiB).
            return Err(invalid(format!(
                "No processor type exists with name [{kind}] in [{field}]{hint}"
            )));
        }
        out.push((kind.as_str(), config));
    }
    Ok(out)
}

/// Checks a processor's options: its own and the common ones.
fn options<'a>(kind: &str, config: &'a Json, own: &[&str]) -> Result<&'a Json, Refused> {
    let Json::Obj(members) = config else {
        // ALLOC: error path; one message.
        return Err(invalid(format!("processor [{kind}] must be an object")));
    };
    if let Some((k, _)) = members
        .iter()
        .find(|(k, _)| !COMMON.contains(&k.as_str()) && !own.contains(&k.as_str()))
    {
        // ALLOC: error path; one message.
        return Err(invalid(format!(
            "processor [{kind}] does not support the option [{k}]"
        )));
    }
    match config.get("ignore_failure") {
        None | Some(Json::Bool(false)) => {}
        Some(Json::Bool(true)) => {
            // ALLOC: error path; one message.
            return Err(unsupported(&format!(
                "[ignore_failure] on processor [{kind}]"
            )));
        }
        Some(_) => {
            // ALLOC: error path; one message.
            return Err(invalid(format!(
                "[ignore_failure] of processor [{kind}] must be a boolean"
            )));
        }
    }
    for key in ["tag", "description"] {
        if config.get(key).is_some_and(|v| !matches!(v, Json::Str(_))) {
            // ALLOC: error path; one message.
            return Err(invalid(format!(
                "[{key}] of processor [{kind}] must be a string"
            )));
        }
    }
    Ok(config)
}

fn prefix(kind: &str, config: &Json) -> Result<String, Refused> {
    match config.get("context_prefix") {
        // ALLOC: compiled once a search that names the pipeline; bounded by its definition (at most 1 MiB).
        None => Ok(String::new()),
        // ALLOC: compiled once a search that names the pipeline.
        Some(Json::Str(s)) => Ok(s.clone()),
        Some(_) => {
            // ALLOC: error path; one message.
            Err(invalid(format!(
                "[context_prefix] of processor [{kind}] must be a string"
            )))
        }
    }
}

fn request_step(kind: &str, config: &Json) -> Result<RequestStep, Refused> {
    match kind {
        "oversample" => {
            let config = options(kind, config, &["sample_factor", "context_prefix"])?;
            let factor = match config.get("sample_factor") {
                Some(Json::Num(n)) => n.parse::<f64>().ok(),
                Some(Json::Str(s)) => s.parse::<f64>().ok(),
                _ => None,
            }
            .filter(|f| f.is_finite())
            .ok_or_else(|| invalid("[sample_factor] required property is missing or not a number in processor [oversample]"))?;
            if factor < 1.0 {
                return Err(invalid(
                    "[sample_factor] Value must be >= 1.0 in processor [oversample]",
                ));
            }
            Ok(RequestStep::Oversample {
                factor,
                prefix: prefix(kind, config)?,
            })
        }
        _ => {
            let config = options(kind, config, &["query"])?;
            let Some(query @ Json::Obj(_)) = config.get("query") else {
                return Err(invalid(
                    "Did not specify the query property in processor of type filter_query",
                ));
            };
            Ok(RequestStep::Filter(parse_query(query)?))
        }
    }
}

fn response_step(kind: &str, config: &Json) -> Result<ResponseStep, Refused> {
    let config = options(kind, config, &["target_size", "context_prefix"])?;
    let target = match config.get("target_size") {
        None => None,
        Some(Json::Num(n)) => match n.parse::<i64>() {
            Ok(v) if v >= 0 => Some(usize::try_from(v).unwrap_or(usize::MAX)),
            _ => {
                return Err(invalid(
                    "[target_size] Value must be >= 0 and whole in processor [truncate_hits]",
                ));
            }
        },
        Some(_) => {
            return Err(invalid(
                "[target_size] must be a whole number in processor [truncate_hits]",
            ));
        }
    };
    Ok(ResponseStep::Truncate {
        target,
        prefix: prefix(kind, config)?,
    })
}

/// A query object as the search parser reads it.
fn parse_query(query: &Json) -> Result<Query, Refused> {
    // ALLOC: compiled once a search that names the pipeline; bounded by its definition (at most 1 MiB).
    let text = format!("{{\"query\":{}}}", query.to_json());
    let mut rec = Recorder::new(1 << 20);
    let mut t = Tokenizer::new(Limits::default());
    t.feed(text.as_bytes(), &mut rec)
        .and_then(|()| t.finish(&mut rec))
        // ALLOC: error path; one message.
        .map_err(|e| {
            invalid(format!(
                "could not read the filter query: {}",
                e.into_failure().reason
            ))
        })?;
    let source = SearchSource::parse(&rec)
        // ALLOC: error path; one message.
        .map_err(|e| {
            invalid(format!(
                "could not parse the filter query: {}",
                e.into_failure().reason
            ))
        })?;
    source
        .query
        .ok_or_else(|| invalid("the filter query is empty"))
}

//! `scripted_metric` (`ScriptedMetricAggregationBuilder`): four Painless
//! scripts and their `params`. `init_script` builds the shard's `state`,
//! `map_script` folds each matched document into it (run by the engine),
//! `combine_script` turns it into the shard's value, and `reduce_script`
//! turns every shard's value into the aggregation's `value`. Served only
//! when the node compiles Painless ([`crate::painless_switch`]). See
//! docs/specs/scripted-metric.

use std::io::{self, Write};
use std::sync::Arc;

use lucene_engine::painless_score::{PainlessMetricMap, decode_spec};
use lucene_engine::query_blob::Cursor;
pub use lucene_painless::ir::Constant;
use lucene_painless::scripted_metric as run;

use super::MetricAgg;
use super::double::write_java_double;
use super::script_values;
use crate::aggs::partial::Value;
use crate::dsl::error::DslError;
use crate::dsl::queries::function_score_painless::encode;
use crate::dsl::queries::function_score_script::{Lang, Param, ScriptScore, parse_params};
use crate::dsl::tokens::{Kind, Parser};
use crate::failure::Failure;
use crate::mapping::Mapping;
use crate::search::ShardError;

/// The deepest a state's lists and maps may nest when written.
const MAX_DEPTH: usize = 100;

/// A parsed `scripted_metric`.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptedMetricSpec {
    /// `init_script`, optional.
    pub init: Option<ScriptScore>,
    /// `map_script`, run for each matched document.
    pub map: ScriptScore,
    /// `combine_script`, run once on the shard.
    pub combine: ScriptScore,
    /// `reduce_script`, run once on the coordinator.
    pub reduce: ScriptScore,
    /// The aggregation's own `params`, shared by every script.
    pub params: Vec<(String, Param)>,
}

/// What one shard answers, and what a reduce of several holds: the reduce
/// script and every shard's combined value, in shard order.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptedMetricResult {
    /// The `reduce_script`'s source.
    pub reduce_source: String,
    /// Its `params`.
    pub reduce_params: Constant,
    /// Each shard's `combine_script` value.
    pub states: Vec<Constant>,
    /// The aggregation's value once [`Self::settle`] has run the reduce.
    pub done: Option<Constant>,
}

impl ScriptedMetricResult {
    /// The reduce of `parts`, all of one aggregation: every shard's value
    /// in order, under the first's reduce script.
    pub fn merge(parts: &[&ScriptedMetricResult]) -> Option<ScriptedMetricResult> {
        let first = parts.first()?;
        // ALLOC: one value a shard, bounded by the shards.
        let states = parts
            .iter()
            .flat_map(|p| p.states.iter().cloned())
            .collect(); // ALLOC: one value a shard, bounded by the shards.
        Some(ScriptedMetricResult {
            // ALLOC: one source, bounded by the request's script.
            reduce_source: first.reduce_source.clone(),
            reduce_params: first.reduce_params.clone(),
            states,
            done: None,
        })
    }

    /// The aggregation's value: `reduce_script` over the shards' values.
    ///
    /// # Errors
    /// The message of the script's failure.
    pub fn value(&self) -> Result<Constant, String> {
        if let Some(done) = &self.done {
            return Ok(done.clone()); // ALLOC: one value, bounded by the state limit.
        }
        run::reduce(&self.reduce_source, &self.reduce_params, &self.states)
            .map_err(|e| e.to_string()) // ALLOC: error path; one message.
    }
}

impl ScriptedMetricResult {
    /// Runs the reduce script now, so a failure is known before the
    /// response is written and the value is computed once.
    ///
    /// # Errors
    /// The message of the script's failure.
    pub fn settle(&mut self) -> Result<(), String> {
        if self.done.is_none() {
            self.done = Some(self.value()?);
        }
        Ok(())
    }
}

/// `scripted_metric`'s body, the parser on its start.
pub(crate) fn parse_body(p: &mut Parser<'_>, name: &str) -> Result<ScriptedMetricSpec, DslError> {
    let (mut init, mut map, mut combine, mut reduce) = (None, None, None, None);
    let mut params = Vec::new(); // ALLOC: one script each, bounded by the request body.
    let mut key = String::new(); // ALLOC: one entry a param and one key, bounded by the request body.
    let mut key_at = p.location();
    loop {
        let kind = match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                // ALLOC: one key, bounded by the body's recording.
                key = p.text().to_owned();
                key_at = p.location();
                continue;
            }
            Some(kind) => kind,
        };
        let slot = match key.as_str() {
            "init_script" => &mut init,
            "map_script" => &mut map,
            "combine_script" => &mut combine,
            "reduce_script" => &mut reduce,
            "params" if kind == Kind::StartObject => {
                params = parse_params(p)?;
                continue;
            }
            _ => {
                return Err(DslError::xcontent(
                    key_at,
                    // ALLOC: error path; one message.
                    &format!("[scripted_metric] unknown field [{key}]"),
                    None,
                ));
            }
        };
        let script = script_values::parse(p, kind)?;
        if script.lang != Lang::Painless {
            return Err(DslError::unsupported(
                "the [scripted_metric] aggregation's [lang] other than painless",
            ));
        }
        // ALLOC: one script, bounded by its source.
        *slot = Some(script);
    }
    let required = |script: Option<ScriptScore>, field: &str| {
        script.ok_or_else(|| {
            // ALLOC: error path; one message.
            DslError::illegal(format!("[{field}] must not be null: [{name}]"))
        })
    };
    Ok(ScriptedMetricSpec {
        init,
        map: required(map, "mapScript")?,
        combine: required(combine, "combineScript")?,
        reduce: required(reduce, "reduceScript")?,
        params,
    })
}

fn fail(message: impl Into<String>) -> ShardError {
    ShardError {
        status: 400,
        failure: Failure::plain("script_exception", message),
    }
}

/// The aggregation's params under `script`'s own (which win).
fn merged(spec: &ScriptedMetricSpec, script: &ScriptScore) -> ScriptScore {
    // ALLOC: one copy of the params, bounded by the request body.
    let mut params = spec.params.clone();
    for (k, v) in &script.params {
        params.retain(|(name, _)| name != k);
        // ALLOC: one param, bounded by the request body.
        params.push((k.clone(), v.clone()));
    }
    ScriptScore {
        // ALLOC: one source, bounded by the script's size.
        source: script.source.clone(),
        lang: script.lang,
        params,
    }
}

fn params_of(script: &ScriptScore) -> Constant {
    // ALLOC: one entry a param, bounded by the request body.
    Constant::Map(
        script
            .params
            .iter()
            .map(|(k, v)| (Constant::Str(k.clone()), v.constant()))
            .collect(), // ALLOC: one entry a param, bounded by the request body.
    )
}

/// A shard's `scripted_metric` ready to collect: the map script over the
/// state `init_script` left.
pub(crate) struct Prepared {
    map: Arc<PainlessMetricMap>,
}

impl Prepared {
    /// The engine's values source, which runs the map script.
    pub(crate) fn source(&self) -> Arc<PainlessMetricMap> {
        Arc::clone(&self.map)
    }
}

/// Runs `init_script` and sets up `map_script` over `mapping`.
pub(crate) fn prepare(
    spec: &ScriptedMetricSpec,
    mapping: &Mapping,
) -> Result<Prepared, ShardError> {
    let state = match &spec.init {
        Some(init) => {
            let init = merged(spec, init);
            run::init(&init.source, &params_of(&init)).map_err(|e| fail(e.to_string()))? // ALLOC: error path; one message.
        }
        None => Constant::Map(Vec::new()), // ALLOC: empty state.
    };
    let map = merged(spec, &spec.map);
    let blob = encode(mapping, &map, 11).map_err(|failure| ShardError {
        status: 400,
        failure,
    })?;
    let decoded = decode_spec(&mut Cursor::new(&blob))
        .map_err(|_| fail("the map script could not be set up"))?;
    let map = PainlessMetricMap::new(decoded, state).map_err(|e| fail(e.to_string()))?; // ALLOC: error path; one message.
    Ok(Prepared { map: Arc::new(map) })
}

/// The shard's answer once the documents are collected: the state through
/// `combine_script`.
pub(crate) fn finish(
    spec: &ScriptedMetricSpec,
    prepared: &Prepared,
) -> Result<ScriptedMetricResult, ShardError> {
    let state = prepared.map.finish().map_err(|e| fail(e.to_string()))?; // ALLOC: error path; one message.
    let combine = merged(spec, &spec.combine);
    let value = run::combine(&combine.source, &params_of(&combine), &state)
        .map_err(|e| fail(e.to_string()))?; // ALLOC: error path; one message.
    let mut result = empty(spec);
    result.states.push(value);
    Ok(result)
}

/// A shard that held no state: the reduce script, no values.
pub(crate) fn empty(spec: &ScriptedMetricSpec) -> ScriptedMetricResult {
    let reduce = merged(spec, &spec.reduce);
    ScriptedMetricResult {
        reduce_params: params_of(&reduce),
        reduce_source: reduce.source,
        // ALLOC: empty.
        states: Vec::new(),
        done: None,
    }
}

/// The spec of `agg`, which is a `scripted_metric`.
pub(crate) fn spec_of(agg: &MetricAgg) -> Option<&ScriptedMetricSpec> {
    agg.scripted.as_deref()
}

/// The reduce of shard results `parts` of one `scripted_metric`.
pub(crate) fn reduce(parts: &[Value]) -> Option<Value> {
    let results: Vec<&ScriptedMetricResult> = parts
        .iter()
        .map(|p| match p {
            Value::Scripted(r) => Some(&**r),
            _ => None,
        })
        // ALLOC: one entry a shard, bounded by the shards.
        .collect::<Option<_>>()?;
    ScriptedMetricResult::merge(&results).map(|r| Value::Scripted(Box::new(r)))
}

/// `"value":` and the aggregation's value, the reduce script run here.
pub(crate) fn write(out: &mut dyn Write, result: &ScriptedMetricResult) -> io::Result<()> {
    let value = result.value().map_err(io::Error::other)?;
    out.write_all(b"\"value\":")?;
    write_constant(out, &value, 0)
}

fn write_string(out: &mut dyn Write, s: &str) -> io::Result<()> {
    out.write_all(b"\"")?;
    for c in s.chars() {
        match c {
            '"' => out.write_all(b"\\\"")?,
            '\\' => out.write_all(b"\\\\")?,
            '\n' => out.write_all(b"\\n")?,
            '\r' => out.write_all(b"\\r")?,
            '\t' => out.write_all(b"\\t")?,
            c if u32::from(c) < 0x20 => write!(out, "\\u{:04x}", u32::from(c))?,
            c => write!(out, "{c}")?,
        }
    }
    out.write_all(b"\"")
}

fn write_constant(out: &mut dyn Write, c: &Constant, depth: usize) -> io::Result<()> {
    if depth > MAX_DEPTH {
        return Err(io::Error::other(
            "the scripted_metric value nests too deeply",
        ));
    }
    match c {
        Constant::Null => out.write_all(b"null"),
        Constant::Bool(b) => write!(out, "{b}"),
        Constant::Byte(n) => write!(out, "{n}"),
        Constant::Short(n) => write!(out, "{n}"),
        Constant::Int(n) => write!(out, "{n}"),
        Constant::Long(n) => write!(out, "{n}"),
        Constant::Char(n) => write_string(out, &String::from_utf16_lossy(&[*n])),
        Constant::Float(f) => write_double(out, f64::from(*f)),
        Constant::Double(d) => write_double(out, *d),
        Constant::Str(s) | Constant::Pattern(s, _) => write_string(out, s),
        Constant::List(items) => {
            out.write_all(b"[")?;
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.write_all(b",")?;
                }
                write_constant(out, item, depth.saturating_add(1))?;
            }
            out.write_all(b"]")
        }
        Constant::Map(entries) => {
            out.write_all(b"{")?;
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.write_all(b",")?;
                }
                match k {
                    Constant::Str(s) => write_string(out, s)?,
                    other => {
                        let mut key = Vec::new(); // ALLOC: one rendered key, bounded by the state's nesting.
                        write_constant(&mut key, other, depth.saturating_add(1))?;
                        write_string(out, &String::from_utf8_lossy(&key))?;
                    }
                }
                out.write_all(b":")?;
                write_constant(out, v, depth.saturating_add(1))?;
            }
            out.write_all(b"}")
        }
    }
}

fn write_double(out: &mut dyn Write, d: f64) -> io::Result<()> {
    if d.is_finite() {
        write_java_double(out, d)
    } else {
        out.write_all(b"null")
    }
}

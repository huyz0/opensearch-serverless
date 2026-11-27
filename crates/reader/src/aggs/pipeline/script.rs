//! `bucket_script` and `bucket_selector` over the rendered buckets of the
//! aggregation that holds them (`BucketScriptPipelineAggregator.reduce`,
//! `BucketSelectorPipelineAggregator.reduce`): each bucket's `buckets_path`
//! variables join the script's `params`, and the script, run by this node's
//! Painless, gives the bucket's new value or whether the bucket stays.
//! Served only when the node compiles Painless ([`crate::painless_switch`]).

use std::io;

use lucene_painless::ir::Constant;
use lucene_painless::scripted_metric as run;

use super::PipelineAgg;
use super::compute::resolve;
use super::parent::{Entry, put};
use crate::aggs::metric::write_aggregations_piped;
use crate::aggs::partial::Partial;
use crate::aggs::pipeline::BucketValue;

/// A script that failed, as the `io::Error` of the render carries it, for
/// the caller that answers 400 `script_exception` for it.
#[derive(Debug)]
pub struct ScriptFailure {
    /// The pipeline's name.
    pub name: String,
    /// What went wrong.
    pub message: String,
}

impl std::fmt::Display for ScriptFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.name, self.message)
    }
}

impl std::error::Error for ScriptFailure {}

/// The script failure `error` carries, if it is one.
pub fn failure_of(error: &io::Error) -> Option<&ScriptFailure> {
    error.get_ref()?.downcast_ref::<ScriptFailure>()
}

/// Runs the script pipelines once over the reduced `parts`, so a script
/// that fails is known before a response starts.
///
/// # Errors
///
/// The [`ScriptFailure`] of a script that throws or returns the wrong type.
pub fn settle(
    parts: &[Partial],
    pipelines: &[PipelineAgg],
    typed: bool,
) -> Result<(), ScriptFailure> {
    if !pipelines.iter().any(|p| p.kind.runs_script()) {
        return Ok(());
    }
    let rendered =
        write_aggregations_piped(&mut io::sink(), parts, pipelines, typed, &mut |_, _, _| {
            Ok(())
        });
    match rendered.as_ref().err().and_then(failure_of) {
        // ALLOC: error path; one name and message.
        Some(f) => Err(ScriptFailure {
            name: f.name.clone(),
            message: f.message.clone(),
        }),
        None => Ok(()),
    }
}

/// Wraps a failure of `pipe`'s script.
fn failed(pipe: &PipelineAgg, message: impl Into<String>) -> io::Error {
    io::Error::other(ScriptFailure {
        // ALLOC: error path; one name and message.
        name: pipe.name.clone(),
        message: message.into(),
    })
}

/// A script pipeline's script and what its variables read.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptPipe {
    /// The script's source.
    pub source: String,
    /// The script's own params.
    pub params: Vec<(String, Constant)>,
    /// Each variable's name and what it reads from a bucket.
    pub vars: Vec<(String, BucketValue)>,
}

impl ScriptPipe {
    /// The script's value for `vars`, which win over a param of the same
    /// name (`vars.putAll(script.getParams())`, then each variable).
    fn execute(&self, vars: Vec<(String, f64)>) -> Result<Constant, String> {
        // ALLOC: one entry a param and variable, bounded by the request.
        let mut params: Vec<(Constant, Constant)> = self
            .params
            .iter()
            .filter(|(k, _)| !vars.iter().any(|(v, _)| v == k))
            .map(|(k, v)| (Constant::Str(k.clone()), v.clone()))
            // ALLOC: as above.
            .collect();
        params.extend(
            vars.into_iter()
                .map(|(k, v)| (Constant::Str(k), Constant::Double(v))),
        );
        run::reduce(&self.source, &Constant::Map(params), &[])
            // ALLOC: error path; one message.
            .map_err(|e| format!("runtime error in a pipeline script: {e}"))
    }
}

/// The number a script returned (`Number`), `None` for `null`.
fn number(value: &Constant) -> Result<Option<f64>, String> {
    #[allow(clippy::cast_precision_loss)]
    Ok(Some(match *value {
        Constant::Null => return Ok(None),
        Constant::Byte(v) => f64::from(v),
        Constant::Short(v) => f64::from(v),
        Constant::Int(v) => f64::from(v),
        Constant::Long(v) => v as f64,
        Constant::Float(v) => f64::from(v),
        Constant::Double(v) => v,
        _ => {
            // ALLOC: error path; one message.
            return Err(
                "a [bucket_script] script returned a value that is not a number".to_owned(),
            );
        }
    }))
}

/// `BucketScriptPipelineAggregator.reduce`: a bucket whose variable is a gap
/// (under `skip`) or whose script returns `null` gets no value; any other
/// gets the script's number.
pub(super) fn script(entries: &mut [Entry], pipe: &PipelineAgg, typed: bool) -> io::Result<()> {
    let Some(spec) = &pipe.script else {
        return Ok(());
    };
    for (_, bucket) in entries.iter_mut() {
        // ALLOC: one entry a variable, bounded by the request.
        let mut vars = Vec::with_capacity(spec.vars.len());
        let mut skip = false;
        for (name, value) in &spec.vars {
            let v = resolve(bucket, value, pipe.insert_zeros);
            if v.is_nan() {
                skip = true;
                break;
            }
            vars.push((name.clone(), v)); // ALLOC: one name a variable.
        }
        if skip {
            continue;
        }
        let result = spec.execute(vars).and_then(|v| number(&v));
        if let Some(value) = result.map_err(|m| failed(pipe, m))? {
            put(bucket, pipe, typed, value);
        }
    }
    Ok(())
}

/// `BucketSelectorPipelineAggregator.reduce`: the buckets whose script
/// returns `true`. A gap is not skipped here: its variable is NaN under
/// `skip` and 0 under `insert_zeros`.
pub(super) fn select(entries: &mut Vec<Entry>, pipe: &PipelineAgg) -> io::Result<()> {
    let Some(spec) = &pipe.script else {
        return Ok(());
    };
    let mut kept = Vec::with_capacity(entries.len()); // ALLOC: one entry a bucket, as the host's.
    for entry in std::mem::take(entries) {
        let vars = spec
            .vars
            .iter()
            // ALLOC: one entry a variable, bounded by the request.
            .map(|(name, value)| (name.clone(), resolve(&entry.1, value, pipe.insert_zeros)))
            // ALLOC: as above.
            .collect();
        match spec.execute(vars).map_err(|m| failed(pipe, m))? {
            Constant::Bool(true) => kept.push(entry),
            Constant::Bool(false) => {}
            _ => {
                return Err(failed(
                    pipe,
                    "a [bucket_selector] script returned a value that is not a boolean",
                ));
            }
        }
    }
    *entries = kept;
    Ok(())
}

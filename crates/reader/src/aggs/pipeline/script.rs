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
use crate::aggs::pipeline::BucketValue;

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
    fn execute(&self, vars: Vec<(String, f64)>) -> io::Result<Constant> {
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
            .map_err(|e| io::Error::other(format!("runtime error in a pipeline script: {e}")))
    }
}

/// The number a script returned (`Number`), `None` for `null`.
fn number(value: &Constant) -> io::Result<Option<f64>> {
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
            return Err(io::Error::other(
                "a [bucket_script] script returned a value that is not a number",
            ));
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
        if let Some(value) = number(&spec.execute(vars)?)? {
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
        match spec.execute(vars)? {
            Constant::Bool(true) => kept.push(entry),
            Constant::Bool(false) => {}
            _ => {
                return Err(io::Error::other(
                    "a [bucket_selector] script returned a value that is not a boolean",
                ));
            }
        }
    }
    *entries = kept;
    Ok(())
}

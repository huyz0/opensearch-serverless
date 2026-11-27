//! `moving_fn` over the rendered buckets of its histogram
//! (`MovFnPipelineAggregator.reduce`): each bucket with a value gets the
//! script's result over the window of values before it, the script run by
//! this node's Painless with the window as the `double[]` `values`. Served
//! only when the node compiles Painless ([`crate::painless_switch`]).

use std::io;

use lucene_painless::ir::Constant;
use lucene_painless::scripted_metric as run;

use super::PipelineAgg;
use super::compute::resolve;
use super::parent::{Entry, put};
use super::script::{ScriptPipe, failed, number};

/// Declares `values` from the list `params._mf_values`, ahead of the
/// script's own source.
const PRELUDE: &str = "double[] values = new double[params._mf_values.size()]; \
for (int _mf_i = 0; _mf_i < values.length; _mf_i++) { values[_mf_i] = params._mf_values.get(_mf_i); }\n";

impl ScriptPipe {
    /// The script's value over the window `values`.
    fn over(&self, values: &[f64]) -> Result<f64, String> {
        // ALLOC: one entry a param and a window value, bounded by the request and the host's buckets.
        let mut params: Vec<(Constant, Constant)> = self
            .params
            .iter()
            .map(|(k, v)| (Constant::Str(k.clone()), v.clone()))
            // ALLOC: as above.
            .collect();
        params.push((
            Constant::Str("_mf_values".to_owned()), // ALLOC: one name.
            // ALLOC: the window.
            Constant::List(values.iter().map(|v| Constant::Double(*v)).collect()),
        ));
        // ALLOC: one source, bounded by the script's size.
        let source = format!("{PRELUDE}{}", self.source);
        let result = run::reduce(&source, &Constant::Map(params), &[])
            // ALLOC: error path; one message.
            .map_err(|e| format!("runtime error in a pipeline script: {e}"))?;
        number(&result)?
            // ALLOC: error path; one message.
            .ok_or_else(|| "a [moving_fn] script returned null".to_owned())
    }
}

/// `MovFnPipelineAggregator.clamp`: `at` within `0..=len`.
fn clamp(at: i64, len: usize) -> usize {
    usize::try_from(at.max(0)).map_or(0, |at| at.min(len))
}

/// `MovFnPipelineAggregator.reduce`: the values of the host's buckets that
/// have one are the series, and the bucket at position `i` of them gets the
/// script's result over `i - window + shift` up to `i + shift`; a bucket
/// with no value is left as it is.
pub(super) fn moving(entries: &mut [Entry], pipe: &PipelineAgg, typed: bool) -> io::Result<()> {
    let Some(spec) = &pipe.script else {
        return Ok(());
    };
    let series: Vec<f64> = entries
        .iter()
        .map(|(_, bucket)| resolve(bucket, &pipe.value, pipe.insert_zeros))
        .filter(|v| !v.is_nan())
        // ALLOC: one number a bucket of the host, at most the 65,535-bucket limit.
        .collect();
    let window = i64::try_from(spec.window).unwrap_or(i64::MAX);
    let mut position = 0_i64;
    for (_, bucket) in entries.iter_mut() {
        if resolve(bucket, &pipe.value, pipe.insert_zeros).is_nan() {
            continue;
        }
        let from = clamp(
            position.saturating_sub(window).saturating_add(spec.shift),
            series.len(),
        );
        let to = clamp(position.saturating_add(spec.shift), series.len());
        let slice = series.get(from..to.max(from)).unwrap_or_default();
        let value = spec.over(slice).map_err(|m| failed(pipe, m))?;
        put(bucket, pipe, typed, value);
        position = position.saturating_add(1);
    }
    Ok(())
}

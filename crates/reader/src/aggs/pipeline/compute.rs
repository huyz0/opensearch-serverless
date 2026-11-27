//! A sibling pipeline aggregation's result over the rendered body of the
//! aggregation it reads (`BucketMetricsPipelineAggregator.doReduce` and its
//! `avg`, `sum`, `min`, `max` and `stats` collectors).

use std::io::{self, Write};

use oss_doc::mapping::json::{Json, parse};
use oss_rest::write_json_str;

use crate::aggs::metric::double::write_java_double;
use crate::aggs::pipeline::{BucketValue, PipelineAgg, PipelineKind};

/// Writes the body of `pipeline`'s result (the members after the braces) from
/// `target`, the rendered body of the aggregation it reads, braces included.
///
/// # Errors
///
/// An [`io::Error`] when the body is too large to read back, or the writer
/// fails.
pub fn write_result(out: &mut dyn Write, pipeline: &PipelineAgg, target: &[u8]) -> io::Result<()> {
    // ALLOC: the rendered buckets of one aggregation, read back whole; the parse refuses more than 8 MiB.
    let tree = parse(target).map_err(|_| {
        io::Error::other("the buckets of a pipeline aggregation's input are too large to read")
    })?;
    let buckets = buckets_of(&tree);
    // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
    let mut keys: Vec<String> = Vec::new();
    // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
    let mut values: Vec<f64> = Vec::new();
    for (key, bucket) in &buckets {
        let value = match resolve(bucket, &pipeline.value, pipeline.insert_zeros) {
            v if !v.is_nan() => v,
            _ => continue,
        };
        // ALLOC: one entry a bucket of the input, at most the 65,535-bucket limit.
        keys.push(key.clone());
        // ALLOC: as above.
        values.push(value);
    }
    match pipeline.kind {
        PipelineKind::Avg => {
            let sum = values.iter().fold(0.0, |a, b| a + b);
            let count = values.len();
            if count == 0 {
                number(out, "value", f64::NAN)
            } else {
                #[allow(clippy::cast_precision_loss)]
                number(out, "value", sum / count as f64)
            }
        }
        PipelineKind::Sum => number(out, "value", values.iter().fold(0.0, |a, b| a + b)),
        PipelineKind::Min | PipelineKind::Max => {
            let min = pipeline.kind == PipelineKind::Min;
            let mut best = if min {
                f64::INFINITY
            } else {
                f64::NEG_INFINITY
            };
            // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
            let mut at: Vec<&String> = Vec::new();
            for (key, value) in keys.iter().zip(&values) {
                let better = if min { *value < best } else { *value > best };
                if better {
                    best = *value;
                    at.clear();
                    at.push(key);
                } else if (*value - best).abs() == 0.0 {
                    at.push(key);
                }
            }
            number(out, "value", best)?;
            out.write_all(b",\"keys\":[")?;
            for (i, key) in at.iter().enumerate() {
                if i > 0 {
                    out.write_all(b",")?;
                }
                write_json_str(out, key)?;
            }
            out.write_all(b"]")
        }
        PipelineKind::Derivative
        | PipelineKind::CumulativeSum
        | PipelineKind::BucketSort
        | PipelineKind::SerialDiff
        | PipelineKind::BucketScript
        | PipelineKind::BucketSelector
        | PipelineKind::MovingFn => Err(io::Error::other(
            "a parent pipeline aggregation is not computed beside its bucket aggregation",
        )),
        PipelineKind::Stats => {
            let count = values.len();
            let sum = values.iter().fold(0.0, |a, b| a + b);
            let min = values.iter().copied().fold(f64::INFINITY, f64::min);
            let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            write!(out, "\"count\":{count},")?;
            let shown = |v: f64| if count == 0 { f64::NAN } else { v };
            number(out, "min", shown(min))?;
            out.write_all(b",")?;
            number(out, "max", shown(max))?;
            out.write_all(b",")?;
            #[allow(clippy::cast_precision_loss)]
            number(out, "avg", shown(sum / count as f64))?;
            out.write_all(b",")?;
            number(out, "sum", sum)
        }
    }
}

/// `"key":` then `value`, or `null` when it is not a finite number.
fn number(out: &mut dyn Write, key: &str, value: f64) -> io::Result<()> {
    write!(out, "\"{key}\":")?;
    if value.is_finite() {
        write_java_double(out, value)
    } else {
        out.write_all(b"null")
    }
}

/// Each bucket of a rendered multi-bucket body as its key text
/// (`getKeyAsString`) and its object, for a list of buckets or keyed ones.
fn buckets_of(tree: &Json) -> Vec<(String, &Json)> {
    match tree.get("buckets") {
        Some(Json::Arr(items)) => items
            .iter()
            .map(|bucket| (key_text(bucket), bucket))
            // ALLOC: one entry a bucket of the input, at most the 65,535-bucket limit.
            .collect(),
        Some(Json::Obj(members)) => members
            .iter()
            .map(|(key, bucket)| (key.clone(), bucket))
            // ALLOC: as above.
            .collect(),
        _ => Vec::new(),
    }
}

fn key_text(bucket: &Json) -> String {
    match bucket.get("key_as_string").or_else(|| bucket.get("key")) {
        Some(Json::Str(text) | Json::Num(text)) => text.clone(),
        // ALLOC: error path or one name a pipeline aggregation, bounded by the request.
        _ => String::new(),
    }
}

/// What `bucket` contributes (`BucketHelpers.resolveBucketValue`): its value,
/// or for a gap (no value, an infinite one, or a bucket with no document read
/// by anything but `_count`) 0 under `insert_zeros` and NaN, to be skipped,
/// otherwise.
pub fn resolve(bucket: &Json, value: &BucketValue, insert_zeros: bool) -> f64 {
    let raw = read(bucket, value);
    let empty = !matches!(value, BucketValue::Count)
        && matches!(bucket.get("doc_count"), Some(Json::Num(n)) if n == "0");
    if raw.is_finite() && !empty {
        raw
    } else if insert_zeros {
        0.0
    } else {
        f64::NAN
    }
}

/// The member `name` of a bucket: written as `name`, or as `type#name` when
/// the keys are typed.
pub fn member<'a>(bucket: &'a Json, name: &str) -> Option<&'a Json> {
    let Json::Obj(members) = bucket else {
        return None;
    };
    members
        .iter()
        .find(|(k, _)| k == name || k.split_once('#').is_some_and(|(_, n)| n == name))
        .map(|(_, v)| v)
}

/// What `bucket` holds for `value`, NaN for none.
fn read(bucket: &Json, value: &BucketValue) -> f64 {
    let found = match value {
        BucketValue::Count => bucket.get("doc_count"),
        BucketValue::Sub { name, metric } => member(bucket, name).and_then(|sub| sub.get(metric)),
        BucketValue::Key => bucket.get("key"),
        BucketValue::Other { .. } => None,
    };
    match found {
        Some(Json::Num(text)) => text.parse().unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

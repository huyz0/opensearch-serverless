//! Parent pipeline aggregations over the rendered body of the bucket
//! aggregation they sit in: `derivative`, `cumulative_sum` and `serial_diff` add a result to
//! each bucket (listed after the bucket's sub-aggregations, in the order the
//! pipelines run), `bucket_sort` sorts and trims the buckets, each as
//! OpenSearch 3.10.0's `PipelineAggregator.reduce` does after the final reduce.

use std::cmp::Ordering;
use std::io;

use oss_doc::mapping::json::{Json, java_cmp, parse};

use crate::aggs::metric::double::write_java_double;
use crate::aggs::pipeline::compute::resolve;
use crate::aggs::pipeline::script;
use crate::aggs::pipeline::{BucketValue, PipelineAgg, PipelineKind};

/// One bucket with its key when the buckets are keyed.
pub(super) type Entry = (Option<String>, Json);

/// `body`, the rendered body of a bucket aggregation (braces included), with
/// `pipelines` (the parent pipelines inside it) applied in the order their
/// paths allow, as the bytes to write in its place. `typed` is whether keys
/// are written as `type#name`.
///
/// # Errors
///
/// An [`io::Error`] when the body is too large to read back, or the
/// pipelines read one another in a cycle.
pub fn apply(body: &[u8], pipelines: &[&PipelineAgg], typed: bool) -> io::Result<Vec<u8>> {
    // ALLOC: the rendered buckets of one aggregation, read back whole; the parse refuses more than 8 MiB.
    let mut tree = parse(body).map_err(|_| {
        io::Error::other("the buckets of a pipeline aggregation's host are too large to read")
    })?;
    let Json::Obj(members) = &mut tree else {
        return Ok(body.to_vec()); // ALLOC: the body, unchanged.
    };
    let Some(at) = members.iter().position(|(k, _)| k == "buckets") else {
        return Ok(body.to_vec()); // ALLOC: as above.
    };
    let keyed = matches!(members[at].1, Json::Obj(_));
    let mut entries: Vec<Entry> = match std::mem::replace(&mut members[at].1, Json::Null) {
        // ALLOC: one entry a bucket of the host, at most the 65,535-bucket limit.
        Json::Arr(items) => items.into_iter().map(|b| (None, b)).collect(),
        // ALLOC: as above.
        Json::Obj(items) => items.into_iter().map(|(k, b)| (Some(k), b)).collect(),
        other => {
            members[at].1 = other;
            return Ok(body.to_vec()); // ALLOC: as above.
        }
    };
    let order = execution_order(pipelines)?;
    for &pipe in &order {
        match pipe.kind {
            PipelineKind::Derivative => derive(&mut entries, pipe, typed),
            PipelineKind::CumulativeSum => accumulate(&mut entries, pipe, typed),
            PipelineKind::SerialDiff => serial_diff(&mut entries, pipe, typed),
            PipelineKind::BucketScript => script::script(&mut entries, pipe, typed)?,
            PipelineKind::BucketSelector => script::select(&mut entries, pipe)?,
            _ => sort(&mut entries, pipe),
        }
    }
    members[at].1 = if keyed {
        // ALLOC: one entry a bucket, as above.
        Json::Obj(
            entries
                .into_iter()
                .map(|(k, b)| (k.unwrap_or_default(), b))
                // ALLOC: one entry or message bounded by the request's pipelines and names.
                .collect(),
        )
    } else {
        // ALLOC: as above.
        Json::Arr(entries.into_iter().map(|(_, b)| b).collect())
    };
    Ok(tree.to_json().into_bytes())
}

/// The pipelines in the order they run: request order, except that one that
/// reads another's result waits for it.
fn execution_order<'a>(pipelines: &[&'a PipelineAgg]) -> io::Result<Vec<&'a PipelineAgg>> {
    // ALLOC: one entry a pipeline, bounded by the request's.
    let mut left: Vec<&PipelineAgg> = pipelines.to_vec();
    // ALLOC: as above.
    let mut done: Vec<&PipelineAgg> = Vec::with_capacity(left.len());
    while !left.is_empty() {
        let ready = left.iter().position(|p| {
            reads(p).iter().all(|name| {
                !left
                    .iter()
                    .any(|q| !std::ptr::eq(*q, *p) && q.name == **name)
            })
        });
        let Some(at) = ready else {
            return Err(io::Error::other(
                "parent pipeline aggregations read one another's results in a cycle",
            ));
        };
        done.push(left.remove(at));
    }
    Ok(done)
}

/// The sub-aggregation names a pipeline reads.
fn reads(pipe: &PipelineAgg) -> Vec<&String> {
    pipe.sort
        .iter()
        .flat_map(|s| s.fields.iter().map(|(value, _)| value))
        .chain(
            pipe.script
                .iter()
                .flat_map(|s| s.vars.iter().map(|(_, v)| v)),
        )
        .chain(std::iter::once(&pipe.value))
        .filter_map(|value| match value {
            BucketValue::Sub { name, .. } => Some(name),
            _ => None,
        })
        // ALLOC: one name a path, bounded by the request's.
        .collect()
}

/// `{"value":v}`, `null` for a value that is not a finite number.
fn simple_value(value: f64) -> Json {
    let number = if value.is_finite() {
        // ALLOC: one number's text.
        let mut text = Vec::new();
        let _ = write_java_double(&mut text, value);
        Json::Num(String::from_utf8(text).unwrap_or_default())
    } else {
        Json::Null
    };
    // ALLOC: one result a bucket.
    Json::Obj(vec![("value".to_owned(), number)])
}

/// Adds the result `value` of `pipe` to `bucket`.
pub(super) fn put(bucket: &mut Json, pipe: &PipelineAgg, typed: bool, value: f64) {
    if let Json::Obj(members) = bucket {
        let key = if typed {
            // ALLOC: one key a bucket.
            format!("{}#{}", pipe.kind.result_type(), pipe.name)
        } else {
            pipe.name.clone() // ALLOC: as above.
        };
        members.push((key, simple_value(value)));
    }
}

/// `DerivativePipelineAggregator.reduce`: from the second bucket on, the
/// difference from the bucket before it, the last value kept even when it
/// is a gap.
fn derive(entries: &mut [Entry], pipe: &PipelineAgg, typed: bool) {
    let mut last: Option<f64> = None;
    for (_, bucket) in entries.iter_mut() {
        let value = resolve(bucket, &pipe.value, pipe.insert_zeros);
        if let Some(last) = last {
            put(bucket, pipe, typed, value - last);
        }
        last = Some(value);
    }
}

/// `CumulativeSumPipelineAggregator.reduce`: the running sum, a bucket
/// with no value adding nothing.
fn accumulate(entries: &mut [Entry], pipe: &PipelineAgg, typed: bool) {
    let mut sum = 0.0;
    for (_, bucket) in entries.iter_mut() {
        let value = resolve(bucket, &pipe.value, true);
        if !value.is_nan() {
            sum += value;
        }
        put(bucket, pipe, typed, sum);
    }
}

/// `SerialDiffPipelineAggregator.reduce`: the difference from the bucket
/// `lag` buckets before, for a bucket and that one both having a value; a
/// gap adds no result there, and counts as a position all the same.
fn serial_diff(entries: &mut [Entry], pipe: &PipelineAgg, typed: bool) {
    let values: Vec<f64> = entries
        .iter()
        .map(|(_, bucket)| resolve(bucket, &pipe.value, pipe.insert_zeros))
        // ALLOC: one number a bucket of the host, at most the 65,535-bucket limit.
        .collect();
    for (at, (_, bucket)) in entries.iter_mut().enumerate() {
        let Some(back) = at.checked_sub(pipe.lag) else {
            continue;
        };
        let (this, lagged) = (values[at], values[back]);
        if !this.is_nan() && !lagged.is_nan() {
            put(bucket, pipe, typed, this - lagged);
        }
    }
}

/// What a bucket sorts by: a number, or a key's text.
enum Sorted {
    Num(f64),
    Text(String),
}

fn compare(a: &Sorted, b: &Sorted) -> Ordering {
    match (a, b) {
        (Sorted::Num(x), Sorted::Num(y)) => x.total_cmp(y),
        (Sorted::Text(x), Sorted::Text(y)) => java_cmp(x, y),
        _ => Ordering::Equal,
    }
}

/// `BucketSortPipelineAggregator.reduce`: buckets with none of the sort
/// values dropped, the rest ordered by the sorts (a bucket lacking a value
/// after the ones that have it), then the window of `from` and `size`.
fn sort(entries: &mut Vec<Entry>, pipe: &PipelineAgg) {
    let Some(spec) = &pipe.sort else {
        return;
    };
    if !spec.fields.is_empty() {
        // ALLOC: one list of sort values a bucket.
        let mut keyed: Vec<(Vec<Option<Sorted>>, Entry)> = std::mem::take(entries)
            .into_iter()
            .map(|entry| {
                let values = spec
                    .fields
                    .iter()
                    .map(|(value, _)| match value {
                        BucketValue::Key => match entry.1.get("key") {
                            Some(Json::Num(n)) => n.parse().ok().map(Sorted::Num),
                            Some(Json::Str(s)) => Some(Sorted::Text(s.clone())),
                            _ => None,
                        },
                        other => {
                            let v = resolve(&entry.1, other, pipe.insert_zeros);
                            (!v.is_nan()).then_some(Sorted::Num(v))
                        }
                    })
                    // ALLOC: one entry or message bounded by the request's pipelines and names.
                    .collect();
                (values, entry)
            })
            // ALLOC: one entry or message bounded by the request's pipelines and names.
            .collect();
        keyed.retain(|(values, _)| values.iter().any(Option::is_some));
        keyed.sort_by(|(a, _), (b, _)| {
            for ((x, y), (_, desc)) in a.iter().zip(b).zip(&spec.fields) {
                let order = match (x, y) {
                    (None, None) => continue,
                    (None, Some(_)) => return Ordering::Greater,
                    (Some(_), None) => return Ordering::Less,
                    (Some(x), Some(y)) if *desc => compare(x, y).reverse(),
                    (Some(x), Some(y)) => compare(x, y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
            Ordering::Equal
        });
        entries.extend(keyed.into_iter().map(|(_, entry)| entry));
    }
    let from = spec.from.min(entries.len());
    entries.drain(..from);
    if let Some(size) = spec.size {
        entries.truncate(size);
    }
}

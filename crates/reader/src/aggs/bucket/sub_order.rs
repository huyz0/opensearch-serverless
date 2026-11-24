//! A `terms`' `order` by a metric sub-aggregation: the path
//! (`AggregationPath.parse`), its check against the sub-aggregations
//! (`Aggregator.resolveSortPathOnValidAgg` and the metrics' bucket
//! comparators) and the sort values (`InternalOrder.Aggregation`), at
//! OpenSearch 3.10.0.

use std::cmp::Ordering;

use crate::aggs::metric::{Metric, MetricAgg};
use crate::aggs::partial::Value;

/// `InternalOrder.Aggregation`: the path of the sub-aggregation to order by,
/// and the direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubOrder {
    /// The path as the request gave it.
    pub path: String,
    /// Whether the smallest value lists first.
    pub ascending: bool,
}

/// `AggregationPath`'s refusal of a path with no element.
pub const EMPTY_PATH: &str = "Invalid path []";

/// One element of an aggregation path: `name`, `name.key` or `name[key]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathElement {
    /// The element as written.
    pub full: String,
    /// The aggregation's name.
    pub name: String,
    /// The metric asked of it, if any.
    pub key: Option<String>,
}

fn invalid_element(element: &str, path: &str) -> String {
    // ALLOC: error path; one message.
    format!("Invalid path element [{element}] in path [{path}]")
}

/// `AggregationPath.parse`: the elements of `path` split at `>` (blanks and
/// empty tokens dropped), the key of each read from `[key]` and, on the last,
/// from after its last `.`.
pub fn parse_path(path: &str) -> Result<Vec<PathElement>, String> {
    // ALLOC: one element a path step, bounded by the request's.
    let tokens: Vec<&str> = path
        .split('>')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        // ALLOC: as above.
        .collect();
    if tokens.is_empty() {
        return Err(EMPTY_PATH.to_owned());
    }
    // ALLOC: one element a path step, as above.
    let mut out = Vec::with_capacity(tokens.len());
    for (at, element) in tokens.iter().enumerate() {
        let last = at.saturating_add(1) == tokens.len();
        // ALLOC: one entry a character, bounded by the request's.
        let chars: Vec<char> = element.chars().collect();
        let length = chars.len();
        let text = |from: usize, to: usize| -> String {
            // ALLOC: one name or key a path step, as above.
            chars[from..to].iter().collect()
        };
        let bracket = chars.iter().rposition(|&c| c == '[');
        let dot = chars.iter().rposition(|&c| c == '.').filter(|_| last);
        let (name, key) = match (bracket, dot) {
            (Some(index), _) => {
                if index == 0 || index.saturating_add(3) > length || chars.last() != Some(&']') {
                    return Err(invalid_element(element, path));
                }
                (
                    text(0, index),
                    Some(text(index.saturating_add(1), length.saturating_sub(1))),
                )
            }
            (None, Some(index)) => {
                if index == 0 || index.saturating_add(2) > length {
                    return Err(invalid_element(element, path));
                }
                (text(0, index), Some(text(index.saturating_add(1), length)))
            }
            (None, None) => (text(0, length), None),
        };
        out.push(PathElement {
            // ALLOC: one element a path step, as above.
            full: (*element).to_owned(),
            name,
            key,
        });
    }
    Ok(out)
}

/// The metrics `stats` answers by name.
const STATS_KEYS: [&str; 5] = ["count", "sum", "min", "max", "avg"];

/// Checks `order` against the `subs` of the `terms` it orders, as the shard
/// does when it builds the aggregator: `Err` is the message of the
/// `AggregationExecutionException`.
pub fn validate(order: &SubOrder, subs: &[MetricAgg]) -> Result<(), String> {
    let elements = parse_path(&order.path)?;
    let joined = elements
        .iter()
        .map(|e| e.full.as_str())
        .collect::<Vec<_>>()
        .join(">");
    // ALLOC: error path; one message.
    let wrap = |reason: &str| format!("Invalid aggregation order path [{joined}]. {reason}");
    let Some(first) = elements.first() else {
        return Err(EMPTY_PATH.to_owned());
    };
    let Some(sub) = subs.iter().find(|s| s.name == first.name) else {
        // ALLOC: error path; one message.
        return Err(wrap(&format!(
            "The provided aggregation [{}] either does not exist, or is a pipeline aggregation \
             and cannot be used to sort the buckets.",
            first.full
        )));
    };
    const BUCKETS: &str = "Buckets can only be sorted on a sub-aggregator path that is built out \
                           of zero or more single-bucket aggregations within the path and a final \
                           single-bucket or a metrics aggregation at the path end.";
    if elements.len() > 1 {
        if first.key.is_some() {
            // ALLOC: error path; one message.
            return Err(wrap(&format!(
                "Key only allowed on last aggregation path element but got [{}]",
                first.full
            )));
        }
        // ALLOC: error path; one message.
        return Err(wrap(&format!(
            "{BUCKETS} [{}] is not single-bucket.",
            sub.name
        )));
    }
    let key = first.key.as_deref();
    match sub.metric {
        Metric::Terms => Err(wrap(BUCKETS)),
        Metric::Stats => match key {
            None => Err(wrap(
                "When ordering on a multi-value metrics aggregation a metric name must be \
                 specified.",
            )),
            // ALLOC: error path; one message.
            Some(k) if !STATS_KEYS.contains(&k) => Err(wrap(&format!(
                "Unknown metric name [{k}] on multi-value metrics aggregation [{}]",
                sub.name
            ))),
            Some(_) => Ok(()),
        },
        _ => match key {
            None | Some("value") => Ok(()),
            // ALLOC: error path; one message.
            Some(_) => Err(wrap(&format!(
                "Ordering on a single-value metrics aggregation can only be done on its value. \
                 Either drop the key (a la \"{0}\") or change it to \"value\" (a la \"{0}.value\")",
                sub.name
            ))),
        },
    }
}

/// The value of `value` the order sorts by: the metric itself, or `key` of a
/// `stats`. NaN for an `avg` of nothing.
pub fn sort_value(value: &Value, key: Option<&str>) -> f64 {
    // ARITH: none; a count is far below 2^53, so the conversions are exact.
    #[allow(clippy::cast_precision_loss)]
    let count = |n: u64| n as f64;
    let avg = |sum: f64, n: u64| sum / count(n);
    match *value {
        Value::Min(v) | Value::Max(v) | Value::Sum(v) => v,
        Value::Avg { sum, count: n } => avg(sum, n),
        Value::ValueCount(n) => count(n),
        Value::Stats {
            count: n,
            sum,
            min,
            max,
        } => match key {
            Some("count") => count(n),
            Some("sum") => sum,
            Some("min") => min,
            Some("max") => max,
            _ => avg(sum, n),
        },
        _ => f64::NAN,
    }
}

/// `Comparators.compareDiscardNaN`: NaN sorts after every number, in either
/// direction.
pub fn compare(ascending: bool, a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) if ascending => a.total_cmp(&b),
        (false, false) => b.total_cmp(&a),
    }
}

/// The indexes of `terms` in the order of `order`: by `values` (one a term),
/// ties by term ascending (`BucketOrder.compound`'s tie-breaker).
pub fn ranked<T: Ord>(order: &SubOrder, values: &[f64], terms: &[T]) -> Vec<usize> {
    // ALLOC: one index a bucket, bounded by the buckets.
    let mut at: Vec<usize> = (0..terms.len()).collect();
    at.sort_by(|&a, &b| {
        compare(
            order.ascending,
            values.get(a).copied().unwrap_or(f64::NAN),
            values.get(b).copied().unwrap_or(f64::NAN),
        )
        .then_with(|| terms.get(a).cmp(&terms.get(b)))
    });
    at
}

/// Where `order` points in sub-aggregations named `names`: the index of the
/// sub-aggregation and the key asked of it.
pub fn target<'a>(
    order: &'a SubOrder,
    names: impl Iterator<Item = &'a str>,
) -> Option<(usize, Option<String>)> {
    let first = parse_path(&order.path).ok()?.into_iter().next()?;
    let at = names.enumerate().find(|(_, n)| *n == first.name)?.0;
    Some((at, first.key))
}

//! A `terms`' `order` by a metric sub-aggregation: the path
//! (`AggregationPath.parse`), its check against the sub-aggregations
//! (`Aggregator.resolveSortPathOnValidAgg` and the metrics' bucket
//! comparators) and the sort values (`InternalOrder.Aggregation`), at
//! OpenSearch 3.10.0.

use std::cmp::Ordering;

use crate::aggs::bucket::filters::FilterKind;
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

/// What the check of an order path refused: the message of the
/// `AggregationExecutionException` and the `IllegalArgumentException` it
/// carries, if any.
#[derive(Debug, PartialEq, Eq)]
pub struct Refusal {
    /// The message of the exception.
    pub message: String,
    /// The reason of its cause.
    pub cause: Option<String>,
}

/// Checks `order` against the `subs` of the `terms` it orders, as the shard
/// does when it builds the aggregator: `Err` is the message of the
/// `AggregationExecutionException`.
pub fn validate(order: &SubOrder, subs: &[MetricAgg]) -> Result<(), Refusal> {
    let elements = parse_path(&order.path).map_err(|message| Refusal {
        message,
        cause: None,
    })?;
    let joined = elements
        .iter()
        .map(|e| e.full.as_str())
        .collect::<Vec<_>>()
        .join(">");
    // ALLOC: error path; one message.
    let wrap = |reason: &str| Refusal {
        message: format!("Invalid aggregation order path [{joined}]. {reason}"),
        cause: Some(reason.to_owned()),
    };
    const BUCKETS: &str = "Buckets can only be sorted on a sub-aggregator path that is built out \
                           of zero or more single-bucket aggregations within the path and a final \
                           single-bucket or a metrics aggregation at the path end.";
    let mut level = subs;
    for (at, element) in elements.iter().enumerate() {
        let Some(sub) = level.iter().find(|s| s.name == element.name) else {
            // ALLOC: error path; one message.
            return Err(wrap(&format!(
                "The provided aggregation [{}] either does not exist, or is a pipeline \
                 aggregation and cannot be used to sort the buckets.",
                element.full
            )));
        };
        if at.saturating_add(1) < elements.len() {
            if element.key.is_some() {
                // ALLOC: error path; one message.
                return Err(wrap(&format!(
                    "Key only allowed on last aggregation path element but got [{}]",
                    element.full
                )));
            }
            if !is_single_bucket(sub.metric) {
                // ALLOC: error path; one message.
                return Err(wrap(&format!(
                    "{BUCKETS} [{}] is not single-bucket.",
                    sub.name
                )));
            }
            level = &sub.subs;
            continue;
        }
        return validate_end(sub, element.key.as_deref(), &wrap, BUCKETS);
    }
    Err(wrap(EMPTY_PATH))
}

/// Whether `metric` is a single-bucket aggregation, which a path may pass
/// through and end on (its `doc_count`).
fn is_single_bucket(metric: Metric) -> bool {
    matches!(
        metric,
        Metric::Filter | Metric::Missing | Metric::Nested | Metric::ReverseNested | Metric::Global
    )
}

/// The last element of a path: `sub` with the `key` asked of it.
fn validate_end(
    sub: &MetricAgg,
    key: Option<&str>,
    wrap: &dyn Fn(&str) -> Refusal,
    buckets: &str,
) -> Result<(), Refusal> {
    if is_single_bucket(sub.metric) {
        return match key {
            None | Some("doc_count") => Ok(()),
            // ALLOC: error path; one message.
            Some(_) => Err(wrap(&format!(
                "Ordering on a single-bucket aggregation can only be done on its doc_count. \
                 Either drop the key (a la \"{0}\") or change it to \"doc_count\" \
                 (a la \"{0}.doc_count\") or \"key\".",
                sub.name
            ))),
        };
    }
    match sub.metric {
        Metric::Terms => Err(wrap(buckets)),
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

/// Where `order` points in the sub-aggregations of a bucket: the index of
/// the first element's sub-aggregation, and the whole path.
#[derive(Debug)]
pub struct Target {
    at: usize,
    path: Vec<PathElement>,
}

/// Where `order` points in sub-aggregations named `names`.
pub fn target<'a>(order: &SubOrder, names: impl Iterator<Item = &'a str>) -> Option<Target> {
    let path = parse_path(&order.path).ok()?;
    let first = path.first()?;
    let at = names.enumerate().find(|(_, n)| *n == first.name)?.0;
    Some(Target { at, path })
}

impl Target {
    /// The value a bucket sorts by, from the values of its sub-aggregations
    /// in `list`: NaN when the path leads nowhere.
    pub fn sort(&self, list: &[Value]) -> f64 {
        list.get(self.at)
            .map_or(f64::NAN, |value| self.sort_of(value))
    }

    /// The index of the sub-aggregation the path starts at.
    pub fn index(&self) -> usize {
        self.at
    }

    /// The value a bucket sorts by, from the value of the sub-aggregation
    /// the path starts at.
    pub fn sort_of(&self, value: &Value) -> f64 {
        walk(
            value,
            self.path.get(1..).unwrap_or(&[]),
            self.path.first().and_then(|e| e.key.as_deref()),
        )
    }
}

/// The sort value of `value`, with `rest` of the path below it and `key`
/// asked of it: a single-bucket aggregation gives its document count at the
/// path's end and otherwise passes to the sub-aggregation the path names.
fn walk(value: &Value, rest: &[PathElement], key: Option<&str>) -> f64 {
    let Value::Filters(r) = value else {
        return if rest.is_empty() {
            sort_value(value, key)
        } else {
            f64::NAN
        };
    };
    if !matches!(
        r.kind,
        FilterKind::Filter | FilterKind::Missing | FilterKind::Nested | FilterKind::ReverseNested
    ) {
        return f64::NAN;
    }
    let Some((next, below)) = rest.split_first() else {
        // ARITH: none; a count is far below 2^53, so the conversion is exact.
        #[allow(clippy::cast_precision_loss)]
        return r.buckets.first().map_or(f64::NAN, |b| b.1 as f64);
    };
    let Some(at) = r.sub_meta.iter().position(|m| m.0 == next.name) else {
        return f64::NAN;
    };
    r.subs
        .first()
        .and_then(|list| list.get(at))
        .map_or(f64::NAN, |v| walk(v, below, next.key.as_deref()))
}

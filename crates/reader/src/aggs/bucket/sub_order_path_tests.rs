//! The order of a `terms` by a path through single-bucket aggregations
//! (`filter`, `missing`, `nested`, `reverse_nested`): its check and its sort
//! values.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::filters::{FilterKind, FiltersResult};
use super::sub_order::{SubOrder, target, validate};
use crate::aggs::metric::{Metric, MetricAgg};
use crate::aggs::partial::{Format, Value};

fn agg(name: &str, metric: Metric, subs: Vec<MetricAgg>) -> MetricAgg {
    MetricAgg {
        name: name.to_owned(),
        metric,
        field: "n".to_owned(),
        cardinality: Default::default(),
        percentiles: Default::default(),
        sigma: crate::aggs::metric::DEFAULT_SIGMA,
        top_hits: None,
        terms: None,
        histogram: None,
        date_histogram: None,
        auto_date: None,
        subs,
        range: None,
        filters: None,
        nested: None,
        composite: None,
        significant: None,
        rare: None,
        scripted: None,
        script: None,
    }
}

fn order(path: &str) -> SubOrder {
    SubOrder {
        path: path.to_owned(),
        ascending: true,
    }
}

fn request() -> Vec<MetricAgg> {
    vec![
        agg(
            "f",
            Metric::Filter,
            vec![
                agg("s", Metric::Sum, Vec::new()),
                agg("st", Metric::Stats, Vec::new()),
                agg(
                    "g",
                    Metric::Missing,
                    vec![agg("s", Metric::Sum, Vec::new())],
                ),
            ],
        ),
        agg("t", Metric::Terms, Vec::new()),
    ]
}

fn message(path: &str) -> String {
    validate(&order(path), &request())
        .expect_err("refused")
        .message
}

fn single(kind: FilterKind, count: u64, names: &[&str], values: Vec<Value>) -> Value {
    Value::Filters(Box::new(FiltersResult {
        kind,
        single: true,
        buckets: vec![(String::new(), count)],
        sub_meta: names
            .iter()
            .map(|n| ((*n).to_owned(), Format::Raw))
            .collect(),
        subs: vec![values],
        ..FiltersResult::default()
    }))
}

#[test]
fn a_path_through_single_bucket_aggregations_is_accepted() {
    for path in [
        "f",
        "f.doc_count",
        "f>s",
        "f>s.value",
        "f>st.max",
        "f>g",
        "f>g>s",
    ] {
        assert_eq!(validate(&order(path), &request()), Ok(()), "{path}");
    }
}

#[test]
fn a_path_that_leads_nowhere_is_refused_as_stock_refuses_it() {
    assert_eq!(
        message("f>zz"),
        "Invalid aggregation order path [f>zz]. The provided aggregation [zz] either does not \
         exist, or is a pipeline aggregation and cannot be used to sort the buckets."
    );
    assert_eq!(
        message("f>st"),
        "Invalid aggregation order path [f>st]. When ordering on a multi-value metrics \
         aggregation a metric name must be specified."
    );
    assert_eq!(
        message("f>s.x"),
        "Invalid aggregation order path [f>s.x]. Ordering on a single-value metrics aggregation \
         can only be done on its value. Either drop the key (a la \"s\") or change it to \
         \"value\" (a la \"s.value\")"
    );
    assert_eq!(
        message("f.x"),
        "Invalid aggregation order path [f.x]. Ordering on a single-bucket aggregation can only \
         be done on its doc_count. Either drop the key (a la \"f\") or change it to \
         \"doc_count\" (a la \"f.doc_count\") or \"key\"."
    );
    assert_eq!(
        message("f>g.x"),
        "Invalid aggregation order path [f>g.x]. Ordering on a single-bucket aggregation can \
         only be done on its doc_count. Either drop the key (a la \"g\") or change it to \
         \"doc_count\" (a la \"g.doc_count\") or \"key\"."
    );
    assert_eq!(
        message("f[x]>s"),
        "Invalid aggregation order path [f[x]>s]. Key only allowed on last aggregation path \
         element but got [f[x]]"
    );
    assert!(message("t>s").ends_with("[t] is not single-bucket."));
    assert!(message("f>s>x").ends_with("[s] is not single-bucket."));
}

#[test]
fn a_refusal_carries_the_reason_alone_as_its_cause() {
    let refused = validate(&order("f[x]>s"), &request()).expect_err("refused");
    assert_eq!(
        refused.cause.as_deref(),
        Some("Key only allowed on last aggregation path element but got [f[x]]")
    );
    assert!(
        refused
            .message
            .starts_with("Invalid aggregation order path [f[x]>s]. ")
    );
}

#[test]
fn a_bucket_sorts_by_the_value_below_its_single_bucket_aggregations() {
    let inner = single(FilterKind::Missing, 3, &["s"], vec![Value::Sum(7.5)]);
    let outer = single(
        FilterKind::Filter,
        4,
        &["s", "g"],
        vec![Value::Sum(2.0), inner],
    );
    let list = [outer];
    let sorts = |path: &str| {
        target(&order(path), ["f"].into_iter())
            .expect("a target")
            .sort(&list)
    };
    assert_eq!(sorts("f"), 4.0);
    assert_eq!(sorts("f.doc_count"), 4.0);
    assert_eq!(sorts("f>s"), 2.0);
    assert_eq!(sorts("f>g"), 3.0);
    assert_eq!(sorts("f>g>s"), 7.5);
    assert!(sorts("f>zz").is_nan());
    assert!(sorts("f>s>x").is_nan());
    assert!(target(&order("zz>s"), ["f"].into_iter()).is_none());
}

#[test]
fn an_empty_bucket_sorts_as_its_aggregations_say() {
    let empty = single(
        FilterKind::Nested,
        0,
        &["a", "s"],
        vec![Value::Avg { sum: 0.0, count: 0 }, Value::Sum(0.0)],
    );
    let list = [empty];
    let sorts = |path: &str| {
        target(&order(path), ["c"].into_iter())
            .expect("a target")
            .sort(&list)
    };
    assert_eq!(sorts("c"), 0.0);
    assert!(sorts("c>a").is_nan());
    assert_eq!(sorts("c>s"), 0.0);
}

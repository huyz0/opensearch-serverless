//! The order of a `terms` by a metric sub-aggregation: its path, its check
//! and its sort values.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::sub_order::{PathElement, SubOrder, compare, parse_path, ranked, sort_value, validate};
use crate::aggs::metric::{Metric, MetricAgg};
use crate::aggs::partial::Value;
use std::cmp::Ordering;

fn order(path: &str, ascending: bool) -> SubOrder {
    SubOrder {
        path: path.to_owned(),
        ascending,
    }
}

fn sub(name: &str, metric: Metric) -> MetricAgg {
    MetricAgg {
        name: name.to_owned(),
        metric,
        field: "n".to_owned(),
        cardinality: Default::default(),
        percentiles: Default::default(),
        top_hits: None,
        terms: None,
        histogram: None,
        date_histogram: None,
        subs: Vec::new(),
        range: None,
        filters: None,
        nested: None,
        composite: None,
        significant: None,
        rare: None,
        script: None,
    }
}

fn element(full: &str, name: &str, key: Option<&str>) -> PathElement {
    PathElement {
        full: full.to_owned(),
        name: name.to_owned(),
        key: key.map(str::to_owned),
    }
}

// AC-18
#[test]
fn order_paths_parse_as_aggregation_path_parses_them() {
    assert_eq!(parse_path("m"), Ok(vec![element("m", "m", None)]));
    assert_eq!(
        parse_path("st.max"),
        Ok(vec![element("st.max", "st", Some("max"))])
    );
    assert_eq!(
        parse_path("st[max]"),
        Ok(vec![element("st[max]", "st", Some("max"))])
    );
    // A dot splits at its last occurrence, and only in the last element.
    assert_eq!(
        parse_path("a.b.c"),
        Ok(vec![element("a.b.c", "a.b", Some("c"))])
    );
    assert_eq!(
        parse_path(" a.b > c "),
        Ok(vec![element("a.b", "a.b", None), element("c", "c", None)])
    );
    for bad in ["[x]", "a[", "a[]", "a[b", ".a", "a.", ">>"] {
        let e = parse_path(bad).expect_err(bad);
        assert!(e.starts_with("Invalid path"), "{bad}: {e}");
    }
    assert_eq!(
        parse_path("a[b").unwrap_err(),
        "Invalid path element [a[b] in path [a[b]"
    );
}

#[test]
fn bad_order_paths_are_refused_with_opensearchs_messages() {
    let subs = [
        sub("m", Metric::Max),
        sub("st", Metric::Stats),
        sub("t", Metric::Terms),
    ];
    for (path, ok) in [
        ("m", true),
        ("m.value", true),
        ("m[value]", true),
        ("st.avg", true),
        ("st[count]", true),
    ] {
        assert_eq!(validate(&order(path, true), &subs).is_ok(), ok, "{path}");
    }
    let message = |path: &str| validate(&order(path, true), &subs).expect_err(path);
    assert_eq!(
        message("zz"),
        "Invalid aggregation order path [zz]. The provided aggregation [zz] either does not \
         exist, or is a pipeline aggregation and cannot be used to sort the buckets."
    );
    assert_eq!(
        message("m.max"),
        "Invalid aggregation order path [m.max]. Ordering on a single-value metrics aggregation \
         can only be done on its value. Either drop the key (a la \"m\") or change it to \
         \"value\" (a la \"m.value\")"
    );
    assert_eq!(
        message("st"),
        "Invalid aggregation order path [st]. When ordering on a multi-value metrics aggregation \
         a metric name must be specified."
    );
    assert_eq!(
        message("st.median"),
        "Invalid aggregation order path [st.median]. Unknown metric name [median] on multi-value \
         metrics aggregation [st]"
    );
    assert!(message("t").contains("Buckets can only be sorted on a sub-aggregator path"));
    assert!(message("m>x").ends_with("[m] is not single-bucket."));
    assert_eq!(
        message("m[value]>x"),
        "Invalid aggregation order path [m[value]>x]. Key only allowed on last aggregation path \
         element but got [m[value]]"
    );
}

// AC-17
#[test]
fn the_value_of_a_metric_for_sorting() {
    assert_eq!(sort_value(&Value::Max(4.5), None), 4.5);
    assert_eq!(sort_value(&Value::ValueCount(7), None), 7.0);
    assert_eq!(sort_value(&Value::Avg { sum: 9.0, count: 3 }, None), 3.0);
    assert!(sort_value(&Value::Avg { sum: 0.0, count: 0 }, None).is_nan());
    let stats = Value::Stats {
        count: 4,
        sum: 20.0,
        min: 1.0,
        max: 9.0,
    };
    let by = |key| sort_value(&stats, Some(key));
    assert_eq!(
        [by("count"), by("sum"), by("min"), by("max"), by("avg")],
        [4.0, 20.0, 1.0, 9.0, 5.0]
    );
}

#[test]
fn nan_sorts_last_in_both_directions_and_ties_list_by_term() {
    for ascending in [true, false] {
        assert_eq!(compare(ascending, f64::NAN, 1.0), Ordering::Greater);
        assert_eq!(compare(ascending, 1.0, f64::NAN), Ordering::Less);
        assert_eq!(compare(ascending, f64::NAN, f64::NAN), Ordering::Equal);
    }
    assert_eq!(compare(true, 1.0, 2.0), Ordering::Less);
    assert_eq!(compare(false, 1.0, 2.0), Ordering::Greater);
    assert_eq!(
        compare(true, f64::NEG_INFINITY, f64::INFINITY),
        Ordering::Less
    );
    let terms = ["d", "c", "b", "a"];
    let values = [1.0, f64::NAN, 1.0, 5.0];
    assert_eq!(ranked(&order("m", false), &values, &terms), [3, 2, 0, 1]);
    assert_eq!(ranked(&order("m", true), &values, &terms), [2, 0, 3, 1]);
}

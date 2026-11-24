//! A `composite` parsed, reduced and written out.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::composite::{
    CompositeResult, CompositeSpec, KeyPart, MissingOrder, SourceKind, SourceSpec, reduce,
    reduce_results,
};
use super::composite_render::write_composite;
use crate::aggs::metric::Metric;
use crate::aggs::partial::Value;
use crate::dsl::parse_chunks;

fn source(name: &str, desc: bool, missing: Option<MissingOrder>) -> SourceSpec {
    SourceSpec {
        name: name.to_owned(),
        field: name.to_owned(),
        kind: SourceKind::Terms,
        desc,
        missing_bucket: missing.is_some(),
        missing_order: missing.unwrap_or(MissingOrder::Default),
    }
}

fn result(
    sources: Vec<SourceSpec>,
    size: usize,
    buckets: Vec<(Vec<KeyPart>, u64)>,
) -> CompositeResult {
    CompositeResult {
        spec: CompositeSpec {
            sources,
            size,
            after: None,
        },
        buckets,
    }
}

fn long(n: i64) -> KeyPart {
    KeyPart::Long(n)
}

fn term(t: &str) -> KeyPart {
    KeyPart::Term(t.as_bytes().to_vec())
}

fn written(r: &CompositeResult) -> String {
    let mut out = Vec::new();
    write_composite(&mut out, r).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

fn spec(aggs: &str) -> CompositeSpec {
    let body = format!(r#"{{"aggs":{{"c":{{"composite":{aggs}}}}}}}"#);
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    let agg = source.aggs.first().expect("one aggregation");
    assert_eq!(agg.metric, Metric::Composite);
    agg.composite.as_deref().cloned().expect("a spec")
}

fn reason(aggs: &str) -> String {
    let body = format!(r#"{{"aggs":{{"c":{{"composite":{aggs}}}}}}}"#);
    let failure = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    let mut text = failure.reason.clone();
    let mut cause = failure.cause.as_deref();
    while let Some(c) = cause {
        text.push_str(" | ");
        text.push_str(&c.reason);
        cause = c.cause.as_deref();
    }
    text
}

// AC-6
#[test]
fn equal_keys_add_and_the_buckets_are_cut_at_size() {
    let s = vec![source("a", false, None)];
    let one = result(
        s.clone(),
        2,
        vec![(vec![term("a")], 5), (vec![term("c")], 1)],
    );
    let two = result(s, 2, vec![(vec![term("a")], 2), (vec![term("b")], 4)]);
    let got = reduce_results(&[&one, &two]).expect("reduces");
    assert_eq!(got.buckets, [(vec![term("a")], 7), (vec![term("b")], 4)]);
}

#[test]
fn a_descending_source_reduces_in_descending_order() {
    let s = vec![source("a", true, None)];
    let one = result(s.clone(), 5, vec![(vec![long(9)], 1), (vec![long(2)], 1)]);
    let two = result(s, 5, vec![(vec![long(5)], 3)]);
    let got = reduce_results(&[&one, &two]).expect("reduces");
    let keys: Vec<_> = got.buckets.iter().map(|b| b.0[0].clone()).collect();
    assert_eq!(keys, [long(9), long(5), long(2)]);
}

#[test]
fn several_sources_order_by_the_first_then_the_next() {
    let s = vec![source("a", false, None), source("b", true, None)];
    let one = result(
        s.clone(),
        10,
        vec![(vec![term("x"), long(1)], 1), (vec![term("y"), long(1)], 1)],
    );
    let two = result(s, 10, vec![(vec![term("x"), long(7)], 1)]);
    let got = reduce_results(&[&one, &two]).expect("reduces");
    let keys: Vec<_> = got.buckets.iter().map(|b| b.0.clone()).collect();
    assert_eq!(
        keys,
        [
            vec![term("x"), long(7)],
            vec![term("x"), long(1)],
            vec![term("y"), long(1)]
        ]
    );
}

// AC-4
#[test]
fn the_missing_key_sorts_by_missing_order_and_the_default_follows_the_order() {
    let missing = |order, desc| {
        let s = vec![source("a", desc, Some(order))];
        let part = result(
            s,
            5,
            vec![
                (vec![long(1)], 1),
                (vec![long(3)], 1),
                (vec![KeyPart::Missing], 1),
            ],
        );
        // A shard answers in its own order; the reduce sorts them all.
        let got = reduce_results(&[&part]).expect("reduces");
        got.buckets
            .iter()
            .map(|b| b.0[0].clone())
            .collect::<Vec<_>>()
    };
    let m = KeyPart::Missing;
    assert_eq!(
        missing(MissingOrder::Default, false),
        [m.clone(), long(1), long(3)]
    );
    assert_eq!(
        missing(MissingOrder::Default, true),
        [long(3), long(1), m.clone()]
    );
    assert_eq!(
        missing(MissingOrder::First, true),
        [m.clone(), long(3), long(1)]
    );
    assert_eq!(missing(MissingOrder::Last, false), [long(1), long(3), m]);
}

// AC-6
#[test]
fn the_reduce_counts_its_buckets_against_the_limit_and_keeps_kinds_apart() {
    let s = vec![source("a", false, None)];
    let one = Value::Composite(Box::new(result(s.clone(), 3, vec![(vec![long(1)], 1)])));
    let two = Value::Composite(Box::new(result(s, 3, vec![(vec![long(1)], 2)])));
    let Some(Value::Composite(got)) = reduce(&[one.clone(), two]) else {
        panic!("reduces");
    };
    assert_eq!(got.buckets, [(vec![long(1)], 3)]);
    assert!(reduce(&[one, Value::Min(1.0)]).is_none());
    assert!(reduce(&[]).is_none());
    let (over, made) = super::running::scoped(|| {
        super::running::add(super::BUCKET_LIMIT);
        let part = Value::Composite(Box::new(result(
            vec![source("a", false, None)],
            3,
            vec![(vec![long(1)], 1)],
        )));
        reduce(&[part])
    });
    assert!(over.is_none());
    assert!(made.is_some());
}

// AC-1
#[test]
fn a_result_is_written_as_opensearch_writes_it() {
    let s = vec![
        source("tag", false, None),
        source("n", false, Some(MissingOrder::Default)),
    ];
    let r = result(
        s.clone(),
        10,
        vec![
            (vec![term("a\"b"), long(3)], 2),
            (vec![term("c"), KeyPart::Missing], 1),
            (vec![term("d"), KeyPart::Double(10.0)], 1),
        ],
    );
    assert_eq!(
        written(&r),
        r#""after_key":{"tag":"d","n":10.0},"buckets":[{"key":{"tag":"a\"b","n":3},"doc_count":2},{"key":{"tag":"c","n":null},"doc_count":1},{"key":{"tag":"d","n":10.0},"doc_count":1}]"#
    );
    // No bucket, no `after_key`; a single shard's answer is cut at size.
    assert_eq!(
        written(&result(s.clone(), 10, Vec::new())),
        r#""buckets":[]"#
    );
    let cut = result(
        s,
        1,
        vec![(vec![term("a"), long(1)], 1), (vec![term("b"), long(1)], 1)],
    );
    assert!(written(&cut).starts_with(r#""after_key":{"tag":"a","n":1},"#));
}

// AC-7
#[test]
fn the_body_is_parsed_into_sources_size_and_after() {
    let s = spec(
        r#"{"size":5,"sources":[{"a":{"terms":{"field":"tag","order":"desc","missing_bucket":true,"missing_order":"last"}}},{"b":{"histogram":{"field":"n","interval":2.5}}}],"after":{"a":"t1","b":null}}"#,
    );
    assert_eq!(s.size, 5);
    assert_eq!(s.sources.len(), 2);
    let a = &s.sources[0];
    assert_eq!(
        (a.name.as_str(), a.field.as_str(), a.desc, a.missing_bucket),
        ("a", "tag", true, true)
    );
    assert_eq!(a.missing_order, MissingOrder::Last);
    assert_eq!(s.sources[1].kind, SourceKind::Histogram { interval: 2.5 });
    assert!(s.after.is_some());
    // The defaults: ten buckets, no `after`.
    let d = spec(r#"{"sources":[{"a":{"terms":{"field":"tag"}}}]}"#);
    assert_eq!(d.size, 10);
    assert!(d.after.is_none());
    assert!(!d.sources[0].desc);
    assert_eq!(d.sources[0].missing_order, MissingOrder::Default);
}

#[test]
fn a_body_opensearch_refuses_is_refused() {
    let one = r#""sources":[{"a":{"terms":{"field":"tag"}}}]"#;
    assert!(reason("{}").contains("Required [sources]"));
    assert!(reason(r#"{"sources":[]}"#).contains("Composite [sources] cannot be null or empty"));
    assert!(
        reason(r#"{"sources":[{"a":{"terms":{"field":"x"}}},{"a":{"terms":{"field":"y"}}}]}"#)
            .contains("Composite source names must be unique, found duplicates: [a]")
    );
    assert!(
        reason(&format!(r#"{{{one},"sizes":1}}"#)).contains("[composite] unknown field [sizes]")
    );
    assert!(
        reason(r#"{"sources":[{"a":{"nope":{"field":"x"}}}]}"#)
            .contains("invalid source type: nope")
    );
    assert!(
        reason(r#"{"sources":[{"a":{"terms":{"field":"x","orders":"asc"}}}]}"#)
            .contains("[terms] unknown field [orders] did you mean [order]?")
    );
    assert!(
        reason(r#"{"sources":[{"a":{"terms":{"field":"x","order":"up"}}}]}"#)
            .contains("Unknown SortOrder [up]")
    );
    assert!(
        reason(r#"{"sources":[{"a":{"terms":{"field":"x","missing_order":"first"}}}]}"#)
            .contains("missing_order require missing_bucket is true")
    );
    assert!(reason(r#"{"sources":[{"a":{"terms":{}}}]}"#).contains("Required one of fields"));
}

// AC-5
#[test]
fn a_histogram_source_needs_a_positive_interval() {
    for body in [
        r#"{"sources":[{"a":{"histogram":{"field":"n"}}}]}"#,
        r#"{"sources":[{"a":{"histogram":{"field":"n","interval":0}}}]}"#,
        r#"{"sources":[{"a":{"histogram":{"field":"n","interval":-2}}}]}"#,
    ] {
        assert!(
            reason(body).contains("[interval] must be greater than 0 for [histogram] source"),
            "{body}"
        );
    }
}

#[test]
fn what_this_node_does_not_serve_is_refused_by_name() {
    for (body, what) in [
        (
            r#"{"sources":[{"a":{"date_histogram":{"field":"d","calendar_interval":"day","format":"yyyy"}}}]}"#,
            "a composite source's [format]",
        ),
        (
            r#"{"sources":[{"a":{"date_histogram":{"field":"d","interval":"day"}}}]}"#,
            "a composite source's [interval]",
        ),
        (
            r#"{"sources":[{"a":{"terms":{"field":"x","script":"1"}}}]}"#,
            "a composite source's [script]",
        ),
        (
            r#"{"sources":[{"a":{"terms":{"field":"x","value_type":"long"}}}]}"#,
            "a composite source's [value_type]",
        ),
        (
            r#"{"size":0,"sources":[{"a":{"terms":{"field":"x"}}}]}"#,
            "a [size] of [0]",
        ),
        (
            r#"{"sources":[{"a":{"terms":{"field":"x"}}}],"after":{"a":true}}"#,
            "an [after] value",
        ),
    ] {
        let got = reason(body);
        assert!(
            got.contains(what) && got.contains("is not supported by this node yet"),
            "{body}: {got}"
        );
    }
    // Under another aggregation a `composite` is not served either.
    let body = r#"{"aggs":{"t":{"terms":{"field":"tag"},"aggs":{"c":{"composite":{"sources":[{"a":{"terms":{"field":"n"}}}]}}}}}}"#;
    let failure = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    assert!(
        failure.reason.contains("[composite] sub-aggregation"),
        "{}",
        failure.reason
    );
}

// AC-10
#[test]
fn a_date_histogram_source_is_parsed_with_its_interval_zone_and_offset() {
    use crate::aggs::bucket::date_histogram::{DateUnit, RoundingKind};
    let s = spec(
        r#"{"sources":[{"d":{"date_histogram":{"field":"ts","calendar_interval":"month","time_zone":"+02:00","offset":"+1h"}}},{"e":{"date_histogram":{"field":"ts","fixed_interval":"90s"}}}]}"#,
    );
    let SourceKind::DateHistogram { rounding } = s.sources[0].kind else {
        panic!("not a date_histogram");
    };
    assert_eq!(rounding.kind, RoundingKind::Unit(DateUnit::Month));
    assert_eq!((rounding.zone_ms, rounding.offset), (7_200_000, 3_600_000));
    let SourceKind::DateHistogram { rounding } = s.sources[1].kind else {
        panic!("not a date_histogram");
    };
    assert_eq!(rounding.kind, RoundingKind::Interval(90_000));
}

#[test]
fn a_date_histogram_source_needs_one_valid_interval() {
    let src = |body: &str| {
        format!(r#"{{"sources":[{{"d":{{"date_histogram":{{"field":"ts",{body}}}}}}}]}}"#)
    };
    assert!(reason(&src(r#""offset":1"#)).contains("Invalid interval specified"));
    assert!(
        reason(&src(r#""calendar_interval":"day","fixed_interval":"1d""#))
            .contains("Cannot use [fixed_interval] with [calendar_interval] configuration option.")
    );
    assert!(
        reason(&src(r#""calendar_interval":"fortnight""#))
            .contains("could not be parsed as a calendar interval")
    );
    assert!(reason(&src(r#""fixed_interval":"0ms""#)).contains("Zero or negative time interval"));
    assert!(
        reason(&src(r#""fixed_interval":"1d","time_zone":"Europe/Paris""#))
            .contains("is not supported by this node yet")
    );
}

//! Parent pipeline aggregations parsed, validated and computed against the recorded Java.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use oss_doc::mapping::json::{Json, parse};

use super::compute::write_result;
use super::parent::apply;
use super::{BucketValue, PipelineAgg, PipelineKind, SortSpec};

const RECORDED: &str = include_str!("../../../../../fixtures/pipeline-aggs/java-parent-3.10.0.txt");
const ORDER: &str = include_str!("../../../../../fixtures/pipeline-aggs/java-order-3.10.0.txt");

/// The rendered body of the recorder's `histogram` of four buckets. `gap`
/// gives the third bucket's `avg` no value; `empty` gives that bucket no
/// documents.
fn histogram(gap: bool, empty: bool) -> String {
    let docs = |n: u32| {
        if empty && n == 2 {
            0
        } else {
            [3, 5, 2, 4][n as usize]
        }
    };
    let s = |n: u32| {
        if empty && n == 2 {
            "0.0"
        } else {
            ["10.0", "25.0", "7.0", "12.0"][n as usize]
        }
    };
    let a = |n: u32| {
        if n == 2 && gap {
            "null"
        } else {
            ["3.0", "3.0", "0.0", "4.0"][n as usize]
        }
    };
    let buckets: Vec<String> = (0..4u32)
        .map(|n| {
            format!(
                r#"{{"key":{}.0,"doc_count":{},"s":{{"value":{}}},"a":{{"value":{}}}}}"#,
                n.saturating_mul(10),
                docs(n),
                s(n),
                a(n)
            )
        })
        .collect();
    // The recorder's "a" is NaN when it has no count; with `empty` that is
    // also null.
    let body = format!(r#"{{"buckets":[{}]}}"#, buckets.join(","));
    if empty {
        body.replace(r#""a":{"value":0.0}"#, r#""a":{"value":null}"#)
    } else {
        body
    }
}

/// The body a recorded line holds for the histogram, braces included.
fn recorded(name: &str) -> String {
    let line = RECORDED
        .lines()
        .find(|l| l.starts_with(&format!("{name} ")))
        .unwrap_or_else(|| panic!("no fixture {name}"));
    let json = line.split_once(' ').expect("json").1;
    json.strip_prefix(r#"{"h":"#)
        .and_then(|j| j.strip_suffix('}'))
        .unwrap_or_else(|| panic!("{json}"))
        .to_owned()
}

fn value(path: &str) -> BucketValue {
    match path {
        "_count" => BucketValue::Count,
        "_key" => BucketValue::Key,
        sub => BucketValue::Sub {
            name: sub.to_owned(),
            metric: "value".to_owned(),
        },
    }
}

fn parent(name: &str, kind: PipelineKind, path: &str, zeros: bool) -> PipelineAgg {
    PipelineAgg {
        name: name.to_owned(),
        kind,
        path: path.to_owned(),
        agg: "h".to_owned(),
        value: value(path),
        insert_zeros: zeros,
        host: Some("h".to_owned()),
        sort: None,
    }
}

fn sorting(zeros: bool, from: usize, size: Option<usize>, fields: &[(&str, bool)]) -> PipelineAgg {
    PipelineAgg {
        sort: Some(SortSpec {
            fields: fields.iter().map(|(p, d)| (value(p), *d)).collect(),
            from,
            size,
        }),
        ..parent("b", PipelineKind::BucketSort, "", zeros)
    }
}

fn applied(pipes: &[&PipelineAgg], body: &str, typed: bool) -> String {
    String::from_utf8(apply(body.as_bytes(), pipes, typed).expect("applies")).expect("utf-8")
}

// AC-1
#[test]
fn derivative_and_cumulative_sum_give_the_bytes_java_wrote() {
    for gap in [false, true] {
        let g = if gap { "gap." } else { "" };
        for path in ["s", "_count", "a"] {
            for (policy, zeros) in [("insert_zeros", true), ("skip", false)] {
                let name = format!("{g}derivative.{path}.{policy}");
                let p = parent("d", PipelineKind::Derivative, path, zeros);
                assert_eq!(
                    applied(&[&p], &histogram(gap, false), false),
                    recorded(&name),
                    "{name}"
                );
            }
            let name = format!("{g}cumulative_sum.{path}");
            let p = parent("c", PipelineKind::CumulativeSum, path, true);
            assert_eq!(
                applied(&[&p], &histogram(gap, false), false),
                recorded(&name),
                "{name}"
            );
        }
    }
}

// AC-2
#[test]
fn a_bucket_with_no_document_is_a_gap_unless_the_path_is_count() {
    for (policy, zeros) in [("insert_zeros", true), ("skip", false)] {
        for path in ["s", "_count"] {
            let name = format!("empty.derivative.{path}.{policy}");
            let p = parent("d", PipelineKind::Derivative, path, zeros);
            assert_eq!(
                applied(&[&p], &histogram(false, true), false),
                recorded(&name),
                "{name}"
            );
        }
    }
    let c = parent("c", PipelineKind::CumulativeSum, "s", true);
    assert_eq!(
        applied(&[&c], &histogram(false, true), false),
        recorded("empty.cumulative_sum.s")
    );
    // A sibling pipeline reads the same gap: the empty bucket is skipped by
    // `avg_bucket` over a sub-aggregation, counted over `_count`.
    for (policy, zeros) in [("insert_zeros", true), ("skip", false)] {
        for (name, path, kind) in [
            ("empty.sum_bucket.s", "s", PipelineKind::Sum),
            ("empty.avg_bucket._count", "_count", PipelineKind::Avg),
        ] {
            let p = PipelineAgg {
                kind,
                agg: "h".to_owned(),
                host: None,
                ..parent("p", kind, path, zeros)
            };
            let mut out = Vec::new();
            write_result(&mut out, &p, histogram(false, true).as_bytes()).expect("writes");
            let line = RECORDED
                .lines()
                .find(|l| l.starts_with(&format!("{name}.{policy} ")))
                .expect("recorded");
            let want = line.split_once(' ').expect("json").1;
            assert_eq!(
                format!(r#"{{"p":{{{}}}}}"#, String::from_utf8(out).unwrap()),
                want,
                "{name}.{policy}"
            );
        }
    }
    // Skipping drops the empty bucket from an average over a sub-aggregation.
    let avg = PipelineAgg {
        kind: PipelineKind::Avg,
        host: None,
        ..parent("p", PipelineKind::Avg, "s", false)
    };
    let mut out = Vec::new();
    write_result(&mut out, &avg, histogram(false, true).as_bytes()).expect("writes");
    assert_eq!(
        String::from_utf8(out).unwrap(),
        r#""value":15.666666666666666"#
    );
}

// AC-3
#[test]
fn bucket_sort_orders_trims_and_drops_as_java_does() {
    let cases: Vec<(&str, PipelineAgg, bool, bool)> = vec![
        (
            "sort.s_desc.from1.size2",
            sorting(false, 1, Some(2), &[("s", true)]),
            false,
            false,
        ),
        (
            "sort.count_asc",
            sorting(false, 0, None, &[("_count", false)]),
            false,
            false,
        ),
        (
            "sort.key_desc",
            sorting(false, 0, None, &[("_key", true)]),
            false,
            false,
        ),
        (
            "sort.none.from1.size2",
            sorting(false, 1, Some(2), &[]),
            false,
            false,
        ),
        (
            "sort.none.from9",
            sorting(false, 9, None, &[]),
            false,
            false,
        ),
        (
            "gap.sort.a_desc.skip",
            sorting(false, 0, None, &[("a", true)]),
            true,
            false,
        ),
        (
            "gap.sort.a_desc.zeros",
            sorting(true, 0, None, &[("a", true)]),
            true,
            false,
        ),
        (
            "gap.sort.a_asc.skip",
            sorting(false, 0, None, &[("a", false)]),
            true,
            false,
        ),
        (
            "sort.two",
            sorting(false, 0, None, &[("_count", false), ("s", true)]),
            false,
            false,
        ),
        (
            "gap.sort.two.skip",
            sorting(false, 0, None, &[("a", true), ("_count", false)]),
            true,
            false,
        ),
        (
            "gap.sort.two.zeros",
            sorting(true, 0, None, &[("_count", false), ("a", true)]),
            true,
            false,
        ),
        (
            "empty.sort.s_desc.skip",
            sorting(false, 0, None, &[("s", true)]),
            false,
            true,
        ),
        (
            "empty.sort.s_desc.insert_zeros",
            sorting(true, 0, None, &[("s", true)]),
            false,
            true,
        ),
        (
            "empty.sort._count_desc.skip",
            sorting(false, 0, None, &[("_count", true)]),
            false,
            true,
        ),
        (
            "empty.sort._count_desc.insert_zeros",
            sorting(true, 0, None, &[("_count", true)]),
            false,
            true,
        ),
    ];
    for (name, pipe, gap, empty) in cases {
        assert_eq!(
            applied(&[&pipe], &histogram(gap, empty), false),
            recorded(name),
            "{name}"
        );
    }
}

// AC-4
#[test]
fn results_follow_the_sub_aggregations_in_the_order_the_pipelines_run() {
    let line = ORDER
        .lines()
        .find(|l| l.starts_with("shards1 "))
        .expect("recorded");
    let tree = parse(line.split_once(' ').unwrap().1.as_bytes()).expect("json");
    let want = tree
        .get("aggregations")
        .and_then(|a| a.get("h"))
        .expect("h")
        .to_json();
    // The recorder's shard results, reduced: the sub-aggregations `zz`, `s` in
    // the order a reduce lists them.
    let bucket = |key: u32, s: f64| {
        format!(r#"{{"key":{key}.0,"doc_count":2,"zz":{{"value":2.0}},"s":{{"value":{s:.1}}}}}"#)
    };
    let body = format!(
        r#"{{"buckets":[{},{},{}]}}"#,
        bucket(0, 1.0),
        bucket(10, 2.0),
        bucket(20, 3.0)
    );
    // `c` is requested after `d` but reads nothing of it; `d` was named
    // first, so it runs first and is listed first.
    let d = parent("d", PipelineKind::Derivative, "s", false);
    let c = parent("c", PipelineKind::CumulativeSum, "s", true);
    assert_eq!(applied(&[&d, &c], &body, false), want);
    // A pipeline that reads another waits for it, whatever the request order.
    let over = parent("e", PipelineKind::CumulativeSum, "d", true);
    let got = applied(&[&over, &d], &body, false);
    assert!(got.contains(r#""d":{"value":1.0},"e":{"value":"#), "{got}");
}

// AC-4
#[test]
fn typed_keys_name_a_result_by_its_type() {
    let d = parent("d", PipelineKind::Derivative, "s", false);
    let body = r#"{"buckets":[{"key":1,"doc_count":1,"sum#s":{"value":1.0}},{"key":2,"doc_count":1,"sum#s":{"value":4.0}}]}"#;
    assert_eq!(
        applied(&[&d], body, true),
        r#"{"buckets":[{"key":1,"doc_count":1,"sum#s":{"value":1.0}},{"key":2,"doc_count":1,"sum#s":{"value":4.0},"simple_value#d":{"value":3.0}}]}"#
    );
}

// AC-4
#[test]
fn keyed_buckets_are_changed_in_place() {
    let c = parent("c", PipelineKind::CumulativeSum, "_count", true);
    let body = r#"{"buckets":{"lo":{"doc_count":2},"hi":{"doc_count":9}}}"#;
    assert_eq!(
        applied(&[&c], body, false),
        r#"{"buckets":{"lo":{"doc_count":2,"c":{"value":2.0}},"hi":{"doc_count":9,"c":{"value":11.0}}}}"#
    );
    // A body with no buckets is left as it is.
    assert_eq!(applied(&[&c], r#"{"x":1}"#, false), r#"{"x":1}"#);
}

fn keys_of(json: &str) -> Vec<String> {
    match parse(json.as_bytes()).expect("json").get("aggregations") {
        Some(Json::Obj(members)) => members.iter().map(|(k, _)| k.clone()).collect(),
        other => panic!("{other:?}"),
    }
}

// AC-5
#[test]
fn sibling_results_are_listed_after_the_aggregations_a_reduce_lists() {
    use crate::aggs::bucket::terms::{TermsResult, TermsSpec};
    use crate::aggs::metric::render::write_aggregations_piped;
    use crate::aggs::partial::{Format, Partial, Value};
    let part = |name: &str, value| Partial {
        name: name.to_owned(),
        format: Format::Raw,
        value,
    };
    let terms = Value::Terms(Box::new(TermsResult {
        spec: TermsSpec {
            size: 10,
            shard_size: 13,
            ..TermsSpec::default()
        },
        buckets: vec![(1, 5), (2, 3)],
        ..TermsResult::default()
    }));
    let parts = [
        part("h", terms),
        part("zz", Value::Sum(1.0)),
        part("m", Value::Sum(1.0)),
    ];
    let sibling = |name: &str, kind| PipelineAgg {
        agg: "h".to_owned(),
        ..parent(name, kind, "_count", false)
    };
    let pipes = [
        PipelineAgg {
            host: None,
            ..sibling("p", PipelineKind::Sum)
        },
        PipelineAgg {
            host: None,
            ..sibling("q", PipelineKind::Avg)
        },
    ];
    let mut out = Vec::new();
    write_aggregations_piped(&mut out, &parts, &pipes, false, &mut |_, _, _| Ok(()))
        .expect("writes");
    let body = format!(
        "{{{}}}",
        String::from_utf8(out)
            .expect("utf-8")
            .trim_start_matches(',')
    );
    let line = ORDER
        .lines()
        .find(|l| l.starts_with("shards1 "))
        .expect("recorded");
    assert_eq!(keys_of(&body), keys_of(line.split_once(' ').unwrap().1));
}

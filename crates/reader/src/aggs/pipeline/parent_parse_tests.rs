//! Parent pipeline aggregations' bodies and validation against the recorded Java.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::{BucketValue, PipelineKind, SortSpec};
use crate::dsl::parse_chunks;

const PARSED: &str =
    include_str!("../../../../../fixtures/pipeline-aggs/java-parent-parse-3.10.0.txt");

fn value(path: &str) -> BucketValue {
    match path {
        "_count" => BucketValue::Count,
        sub => BucketValue::Sub {
            name: sub.to_owned(),
            metric: "value".to_owned(),
        },
    }
}

pub(super) fn request(host: &str, pipes: &str) -> String {
    let host = match host {
        "top" => String::new(),
        "terms" => format!(
            r#""t":{{"terms":{{"field":"f"}},"aggs":{{"s":{{"sum":{{"field":"n"}}}}{pipes}}}}}"#
        ),
        _ => format!(
            r#""h":{{"histogram":{{"field":"n","interval":10}},"aggs":{{"s":{{"sum":{{"field":"n"}}}},"a":{{"avg":{{"field":"n"}}}}{pipes}}}}}"#
        ),
    };
    if host.is_empty() {
        format!(r#"{{"aggs":{{{pipes}}}}}"#)
    } else {
        format!(r#"{{"aggs":{{{host}}}}}"#)
    }
}

/// `,"p":{"type":{"buckets_path":"path"}}`, or at the top level without the comma.
pub(super) fn pipe(host: &str, name: &str, ty: &str, body: &str) -> String {
    let comma = if host == "top" { "" } else { "," };
    format!(r#"{comma}"{name}":{{"{ty}":{{{body}}}}}"#)
}

pub(super) fn path(p: &str) -> String {
    format!(r#""buckets_path":"{p}""#)
}

fn sort_by(p: &str) -> String {
    format!(r#""sort":["{p}"]"#)
}

// AC-6
#[test]
fn a_parent_pipeline_is_validated_as_java_validates_it() {
    let cases: Vec<(&str, &str, String)> = vec![
        (
            "top.derivative",
            "top",
            pipe("top", "p", "derivative", &path("s")),
        ),
        (
            "top.cumulative_sum",
            "top",
            pipe("top", "p", "cumulative_sum", &path("s")),
        ),
        (
            "top.bucket_sort",
            "top",
            pipe("top", "p", "bucket_sort", &sort_by("s")),
        ),
        (
            "histogram.derivative",
            "h",
            pipe("h", "p", "derivative", &path("s")),
        ),
        (
            "histogram.derivative.bad",
            "h",
            pipe("h", "p", "derivative", &path("zz")),
        ),
        (
            "histogram.derivative.count",
            "h",
            pipe("h", "p", "derivative", &path("_count")),
        ),
        (
            "histogram.derivative.key",
            "h",
            pipe("h", "p", "derivative", &path("_key")),
        ),
        (
            "histogram.derivative.deep",
            "h",
            pipe("h", "p", "derivative", &path("a>s")),
        ),
        (
            "histogram.derivative.self",
            "h",
            pipe("h", "p", "derivative", &path("p")),
        ),
        (
            "histogram.derivative.metric",
            "h",
            pipe("h", "p", "derivative", &path("s.value")),
        ),
        (
            "histogram.derivative.nameclash",
            "h",
            pipe("h", "s", "derivative", &path("a")),
        ),
        (
            "histogram.cumulative_sum",
            "h",
            pipe("h", "p", "cumulative_sum", &path("s")),
        ),
        (
            "histogram.cumulative_sum.bad",
            "h",
            pipe("h", "p", "cumulative_sum", &path("zz")),
        ),
        (
            "histogram.cumulative_sum.avgpath",
            "h",
            pipe("h", "p", "cumulative_sum", &path("a")),
        ),
        (
            "histogram.bucket_sort",
            "h",
            pipe("h", "p", "bucket_sort", &sort_by("s")),
        ),
        (
            "histogram.bucket_sort.bad",
            "h",
            pipe("h", "p", "bucket_sort", &sort_by("zz")),
        ),
        (
            "histogram.bucket_sort.none",
            "h",
            pipe("h", "p", "bucket_sort", r#""size":1"#),
        ),
        (
            "terms.derivative",
            "terms",
            pipe("terms", "p", "derivative", &path("s")),
        ),
        (
            "terms.cumulative_sum",
            "terms",
            pipe("terms", "p", "cumulative_sum", &path("s")),
        ),
        (
            "terms.bucket_sort",
            "terms",
            pipe("terms", "p", "bucket_sort", &sort_by("s")),
        ),
        (
            "terms.bucket_sort.bad",
            "terms",
            pipe("terms", "p", "bucket_sort", &sort_by("zz")),
        ),
        (
            "histogram.two",
            "h",
            format!(
                "{}{}",
                pipe("h", "p", "derivative", &path("s")),
                pipe("h", "q", "cumulative_sum", &path("p"))
            ),
        ),
        (
            "histogram.dup",
            "h",
            format!(
                "{}{}",
                pipe("h", "p", "derivative", &path("s")),
                pipe("h", "p", "cumulative_sum", &path("s"))
            ),
        ),
    ];
    for (label, host, pipes) in cases {
        let line = RECORDED_PARSE_VALIDATION(label);
        let got = parse_chunks([request(host, &pipes).as_bytes()])
            .map(|_| ())
            .map_err(|e| e.into_failure().reason);
        match (line.as_str(), &got) {
            ("ok", Ok(())) => {}
            ("ok", Err(reason)) => {
                assert!(
                    reason.contains("not supported by this node yet"),
                    "{label}: {reason}"
                );
            }
            (expected, Err(reason)) => {
                assert_eq!(
                    reason,
                    &format!("Validation Failed: 1: {expected};"),
                    "{label}"
                );
            }
            (expected, Ok(())) => panic!("{label}: accepted, Java says {expected}"),
        }
    }
}

#[allow(non_snake_case)]
fn RECORDED_PARSE_VALIDATION(label: &str) -> String {
    PARSED
        .lines()
        .find_map(|l| l.strip_prefix(&format!("validate {label} ")))
        .unwrap_or_else(|| panic!("no fixture {label}"))
        .to_owned()
}

/// The messages of an exception chain a recorded line prints
/// (`Class: message <- Class: message`), the head's class last.
pub(super) fn java_chain(text: &str) -> Vec<(String, String)> {
    text.split(" <- ")
        .map(|link| {
            let (class, message) = link.split_once(": ").unwrap_or((link, ""));
            (
                class.rsplit('.').next().unwrap_or(class).to_owned(),
                message.to_owned(),
            )
        })
        .collect()
}

pub(super) fn kind_of(class: &str) -> Option<&'static str> {
    Some(match class {
        "ParsingException" => "parsing_exception",
        "XContentParseException" => "x_content_parse_exception",
        "IllegalArgumentException" => "illegal_argument_exception",
        "NumberFormatException" => "number_format_exception",
        _ => return None,
    })
}

// AC-7
#[test]
fn a_body_is_read_as_java_reads_it() {
    let mut checked = 0;
    for line in PARSED.lines().filter(|l| !l.starts_with("validate ")) {
        let (ty, rest) = line.split_once(' ').expect("type");
        let (body, outcome) = rest.split_once(" -> ").expect("outcome");
        let req = request("h", &format!(r#","p":{{"{ty}":{body}}}"#));
        let got = parse_chunks([req.as_bytes()])
            .map(|_| ())
            .map_err(|e| e.into_failure());
        if outcome.starts_with("ok ") {
            if let Err(f) = &got {
                assert!(
                    f.reason.contains("not supported by this node yet"),
                    "{line}: {}",
                    f.reason
                );
            }
            continue;
        }
        let chain = java_chain(outcome);
        let comparable = chain.iter().all(|(c, _)| kind_of(c).is_some());
        let Err(f) = got else {
            panic!("{line}: accepted");
        };
        if !comparable {
            continue;
        }
        let mut at = Some(&f);
        for (class, message) in &chain {
            let held = at.expect("a link");
            assert_eq!(Some(held.kind), kind_of(class), "{line}");
            // The recorder parsed the body alone: a location in the message
            // is where the body starts there, not in the whole request.
            let unlocated = |m: &str| match m.strip_prefix('[').and_then(|r| r.split_once("] ")) {
                Some((at, rest)) if at.contains(':') => rest.to_owned(),
                _ => m.to_owned(),
            };
            assert_eq!(unlocated(&held.reason), unlocated(message), "{line}");
            at = held.cause.as_deref();
        }
        checked += 1;
    }
    assert!(checked > 40, "{checked}");
}

// AC-8
#[test]
fn a_parent_pipeline_under_a_nested_aggregation_or_a_sibling_one_in_a_bucket_is_refused() {
    let nested = r#"{"aggs":{"t":{"terms":{"field":"f"},"aggs":{"h":{"histogram":{"field":"n","interval":10},"aggs":{"d":{"derivative":{"buckets_path":"_count"}}}}}}}}"#;
    let f = parse_chunks([nested.as_bytes()])
        .expect_err("refused")
        .into_failure();
    assert!(
        f.reason.contains("not supported by this node yet"),
        "{}",
        f.reason
    );
    let sibling = r#"{"aggs":{"h":{"histogram":{"field":"n","interval":10},"aggs":{"p":{"avg_bucket":{"buckets_path":"x>y"}}}}}}"#;
    let f = parse_chunks([sibling.as_bytes()])
        .expect_err("refused")
        .into_failure();
    assert!(
        f.reason.contains("not supported by this node yet"),
        "{}",
        f.reason
    );
    let unit = request(
        "h",
        &pipe("h", "p", "derivative", r#""buckets_path":"s","unit":"1d""#),
    );
    let f = parse_chunks([unit.as_bytes()])
        .expect_err("refused")
        .into_failure();
    assert!(
        f.reason.contains("not supported by this node yet"),
        "{}",
        f.reason
    );
}

// AC-9
#[test]
fn the_parent_pipelines_of_a_request_are_kept_with_their_host() {
    let body = request(
        "h",
        &format!(
            "{}{}",
            pipe("h", "d", "derivative", &path("s")),
            pipe(
                "h",
                "b",
                "bucket_sort",
                r#""sort":[{"a":"desc"}],"from":1,"size":2,"gap_policy":"insert_zeros""#
            )
        ),
    );
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    assert_eq!(source.aggs.len(), 1);
    assert_eq!(
        source.aggs[0].subs.len(),
        2,
        "pipelines are not sub-aggregations"
    );
    let [d, b] = source.pipelines.as_slice() else {
        panic!("{:?}", source.pipelines);
    };
    assert_eq!(
        (d.kind, d.host.as_deref(), &d.value, d.insert_zeros),
        (PipelineKind::Derivative, Some("h"), &value("s"), false)
    );
    assert_eq!(b.kind, PipelineKind::BucketSort);
    assert!(b.insert_zeros);
    assert_eq!(
        b.sort,
        Some(SortSpec {
            fields: vec![(value("a"), true)],
            from: 1,
            size: Some(2)
        })
    );
}

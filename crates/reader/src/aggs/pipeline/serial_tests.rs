//! `serial_diff` parsed, validated and computed against the recorded Java.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::PipelineKind;
use super::parent_parse_tests::{java_chain, kind_of, path, pipe, request};
use super::parent_tests::{applied, histogram, parent};
use crate::dsl::parse_chunks;

const COMPUTED: &str = include_str!("../../../../../fixtures/pipeline-aggs/java-serial-3.10.0.txt");
const PARSED: &str =
    include_str!("../../../../../fixtures/pipeline-aggs/java-serial-parse-3.10.0.txt");

/// The body of a recorded line of the histogram, braces included.
fn recorded(name: &str) -> String {
    let line = COMPUTED
        .lines()
        .find(|l| l.starts_with(&format!("{name} ")))
        .unwrap_or_else(|| panic!("no fixture {name}"));
    let json = line.split_once(' ').expect("json").1;
    json.strip_prefix(r#"{"h":"#)
        .and_then(|j| j.strip_suffix('}'))
        .unwrap_or_else(|| panic!("{json}"))
        .to_owned()
}

fn serial(path: &str, lag: usize, zeros: bool) -> super::PipelineAgg {
    let mut p = parent("d", PipelineKind::SerialDiff, path, zeros);
    p.lag = lag;
    p
}

// AC-1
#[test]
fn serial_diff_gives_the_bytes_java_wrote() {
    let mut checked = 0;
    for gap in [false, true] {
        let g = if gap { "gap." } else { "" };
        for path in ["s", "_count", "a"] {
            for lag in [1, 2, 3, 5] {
                for (policy, zeros) in [("insert_zeros", true), ("skip", false)] {
                    let name = format!("{g}serial.{path}.lag{lag}.{policy}");
                    assert_eq!(
                        applied(&[&serial(path, lag, zeros)], &histogram(gap, false), false),
                        recorded(&name),
                        "{name}"
                    );
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 48);
}

// AC-2
#[test]
fn a_bucket_with_no_document_is_a_gap_for_serial_diff() {
    for (policy, zeros) in [("insert_zeros", true), ("skip", false)] {
        for (path, lag) in [("s", 1), ("_count", 2)] {
            let name = format!("empty.serial.{path}.lag{lag}.{policy}");
            assert_eq!(
                applied(&[&serial(path, lag, zeros)], &histogram(false, true), false),
                recorded(&name),
                "{name}"
            );
        }
    }
}

// AC-3
#[test]
fn a_typed_serial_diff_result_is_a_simple_value() {
    let p = serial("s", 1, false);
    let out = applied(&[&p], &histogram(false, false), true);
    assert!(out.contains(r#""simple_value#d":{"value":15.0}"#), "{out}");
}

// AC-4
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
            // A `format` is refused by name; everything else is read.
            match &got {
                Ok(()) => {}
                // A path the request does not hold fails validation, as Java's
                // would for these bodies placed in a request.
                Err(f) => assert!(
                    (f.reason.contains("not supported by this node yet")
                        && (line.contains("\"format\"") || line.contains("3000000000")))
                        || f.reason.contains("No aggregation found for path"),
                    "{line}: {}",
                    f.reason
                ),
            }
            checked += 1;
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
            let unlocated = |m: &str| match m.strip_prefix('[').and_then(|r| r.split_once("] ")) {
                Some((at, rest)) if at.contains(':') => rest.to_owned(),
                _ => m.to_owned(),
            };
            assert_eq!(unlocated(&held.reason), unlocated(message), "{line}");
            at = held.cause.as_deref();
        }
        checked += 1;
    }
    assert!(checked > 25, "{checked}");
}

// AC-5
#[test]
fn serial_diff_is_validated_as_java_validates_it() {
    let cases: Vec<(&str, &str, String)> = vec![
        (
            "top.serial_diff",
            "top",
            pipe("top", "p", "serial_diff", &path("s")),
        ),
        (
            "histogram.serial_diff",
            "h",
            pipe("h", "p", "serial_diff", &path("s")),
        ),
        (
            "histogram.serial_diff.bad",
            "h",
            pipe("h", "p", "serial_diff", &path("zz")),
        ),
        (
            "histogram.serial_diff.count",
            "h",
            pipe("h", "p", "serial_diff", &path("_count")),
        ),
        (
            "histogram.serial_diff.key",
            "h",
            pipe("h", "p", "serial_diff", &path("_key")),
        ),
        (
            "histogram.serial_diff.deep",
            "h",
            pipe("h", "p", "serial_diff", &path("a>s")),
        ),
        (
            "histogram.serial_diff.self",
            "h",
            pipe("h", "p", "serial_diff", &path("p")),
        ),
        (
            "histogram.serial_diff.metric",
            "h",
            pipe("h", "p", "serial_diff", &path("s.value")),
        ),
        (
            "histogram.serial_diff.nameclash",
            "h",
            pipe("h", "s", "serial_diff", &path("a")),
        ),
        (
            "terms.serial_diff",
            "terms",
            pipe("terms", "p", "serial_diff", &path("s")),
        ),
        (
            "histogram.serial_diff.after_derivative",
            "h",
            format!(
                "{}{}",
                pipe("h", "q", "derivative", &path("s")),
                pipe("h", "p", "serial_diff", &path("q"))
            ),
        ),
        (
            "histogram.dup",
            "h",
            format!(
                "{}{}",
                pipe("h", "p", "serial_diff", &path("s")),
                pipe("h", "p", "serial_diff", &path("s"))
            ),
        ),
    ];
    for (label, host, pipes) in cases {
        let line = PARSED
            .lines()
            .find_map(|l| l.strip_prefix(&format!("validate {label} ")))
            .unwrap_or_else(|| panic!("no fixture {label}"));
        let got = parse_chunks([request(host, &pipes).as_bytes()])
            .map(|_| ())
            .map_err(|e| e.into_failure().reason);
        match (line, &got) {
            ("ok", Ok(())) => {}
            ("ok", Err(reason)) => assert!(
                reason.contains("not supported by this node yet"),
                "{label}: {reason}"
            ),
            (expected, Err(reason)) => assert_eq!(
                reason,
                &format!("Validation Failed: 1: {expected};"),
                "{label}"
            ),
            (expected, Ok(())) => panic!("{label}: accepted, Java says {expected}"),
        }
    }
}

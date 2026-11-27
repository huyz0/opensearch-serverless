//! `moving_fn` parsed and computed against the recorded Java
//! (`fixtures/pipeline-script/java-moving-3.10.0.txt`), its scripts the
//! Painless equivalents of the recorder's test engine's.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::parent_parse_tests::{java_chain, kind_of, pipe, request};
use super::parent_tests::{applied, histogram, parent};
use super::script::ScriptPipe;
use super::{BucketValue, PipelineAgg, PipelineKind};
use crate::dsl::parse_chunks;
use lucene_painless::ir::Constant;

const RECORDED: &str =
    include_str!("../../../../../fixtures/pipeline-script/java-moving-3.10.0.txt");

/// The recorder's test script `code` in Painless.
fn painless(code: &str) -> &'static str {
    match code {
        "avg" => "return MovingFunctions.unweightedAvg(values)",
        "sum" => "return MovingFunctions.sum(values)",
        "max" => "return MovingFunctions.max(values)",
        "min" => "return MovingFunctions.min(values)",
        "lw" => "return MovingFunctions.linearWeightedAvg(values)",
        "std" => "return MovingFunctions.stdDev(values, MovingFunctions.unweightedAvg(values))",
        "ewma" => "return MovingFunctions.ewma(values, 0.5)",
        "holt" => "return MovingFunctions.holt(values, 0.5, 0.5)",
        "count" => "return values.length",
        "params" => "return params.k + values.length",
        "boom" => "throw new IllegalStateException('boom')",
        other => panic!("{other}"),
    }
}

fn moving(path: &str, code: &str, window: usize, shift: i64, zeros: bool) -> PipelineAgg {
    let mut agg = parent("p", PipelineKind::MovingFn, path, zeros);
    agg.value = match path {
        "_count" => BucketValue::Count,
        sub => BucketValue::Sub {
            name: sub.to_owned(),
            metric: "value".to_owned(),
        },
    };
    agg.script = Some(Box::new(ScriptPipe {
        source: painless(code).to_owned(),
        params: vec![("k".to_owned(), Constant::Int(3))],
        vars: Vec::new(),
        window,
        shift,
    }));
    agg
}

fn body_of(line: &str) -> String {
    let json = line.split_once(' ').expect("json").1;
    json.strip_prefix(r#"{"h":"#)
        .and_then(|j| j.strip_suffix('}'))
        .expect("body")
        .to_owned()
}

// AC-1
#[test]
fn the_window_is_what_java_gives_each_function() {
    let mut checked = 0;
    for line in RECORDED.lines().filter(|l| l.starts_with(['p', 'g', 'e'])) {
        let name = line.split_once(' ').expect("name").0;
        if name.starts_with("parse") {
            continue;
        }
        let parts: Vec<&str> = name.split('.').collect();
        let [input, path, policy, window, shift, code] = parts[..] else {
            continue;
        };
        let window = window.trim_start_matches('w').parse().expect("window");
        let shift = shift.trim_start_matches('s').parse().expect("shift");
        let p = moving(path, code, window, shift, policy == "insert_zeros");
        let hist = histogram(input == "gap", input == "empty");
        assert_eq!(applied(&[&p], &hist, false), body_of(line), "{name}");
        checked += 1;
    }
    assert!(checked > 300, "{checked}");
}

// AC-2
#[test]
fn a_path_that_names_nothing_leaves_the_buckets_as_they_were() {
    let p = moving("zz", "avg", 2, 0, false);
    let line = RECORDED
        .lines()
        .find(|l| l.starts_with("badpath "))
        .expect("recorded");
    assert_eq!(
        applied(&[&p], &histogram(false, false), false),
        body_of(line)
    );
}

// AC-3
#[test]
fn a_typed_result_is_a_simple_value() {
    let p = moving("s", "count", 2, 1, false);
    let out = applied(&[&p], &histogram(false, false), true);
    assert!(out.contains(r#""simple_value#p":{"value":1.0}"#), "{out}");
}

// AC-4
#[test]
fn a_script_that_throws_fails_the_search() {
    let p = moving("s", "boom", 2, 0, false);
    let body = histogram(false, false);
    let e = super::parent::apply(body.as_bytes(), &[&p], false).expect_err("fails");
    assert_eq!(
        super::script::failure_of(&e).map(|f| f.name.as_str()),
        Some("p")
    );
}

fn parse_with(body: &str) -> Result<Vec<PipelineAgg>, crate::failure::Failure> {
    crate::painless_switch::set_for_this_thread(true);
    let req = request("h", &pipe("h", "p", "moving_fn", body));
    parse_chunks([req.as_bytes()])
        .map(|s| s.pipelines)
        .map_err(|e| e.into_failure())
}

// AC-5
#[test]
fn a_body_is_read_as_java_reads_it() {
    let mut checked = 0;
    for line in RECORDED.lines().filter(|l| l.starts_with("parse ")) {
        let rest = line.strip_prefix("parse ").expect("prefix");
        let (body, outcome) = rest.split_once(" -> ").expect("outcome");
        let body = body
            .strip_prefix('{')
            .and_then(|b| b.strip_suffix('}'))
            .expect("object");
        let got = parse_with(body);
        if outcome.starts_with("ok ") {
            if let Err(f) = &got {
                assert!(
                    f.reason.contains("not supported by this node yet")
                        || f.reason.contains("runs only lang"),
                    "{line}: {}",
                    f.reason
                );
            }
            continue;
        }
        let chain = java_chain(outcome);
        let Err(f) = got else {
            panic!("{line}: accepted")
        };
        if !chain.iter().all(|(c, _)| kind_of(c).is_some()) {
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
    assert!(checked > 10, "{checked}");
}

// AC-6
#[test]
fn a_parsed_moving_fn_keeps_its_window_shift_and_host() {
    let pipes = parse_with(
        r#""buckets_path":"s","window":3,"shift":-1,"script":"return values.length","gap_policy":"insert_zeros""#,
    )
    .expect("parses");
    let p = &pipes[0];
    assert_eq!(p.kind, PipelineKind::MovingFn);
    assert_eq!(p.host.as_deref(), Some("h"));
    assert!(p.insert_zeros);
    let s = p.script.as_ref().expect("script");
    assert_eq!((s.window, s.shift), (3, -1));
}

// AC-7
#[test]
fn the_switch_gates_it_and_only_a_histogram_holds_it() {
    crate::painless_switch::set_for_this_thread(false);
    let body = r#""buckets_path":"s","window":2,"script":"1""#;
    let off = request("h", &pipe("h", "p", "moving_fn", body));
    let f = parse_chunks([off.as_bytes()])
        .expect_err("off")
        .into_failure();
    assert!(
        f.reason.contains("not supported by this node yet"),
        "{}",
        f.reason
    );
    crate::painless_switch::set_for_this_thread(true);
    let top = request("top", &pipe("top", "p", "moving_fn", body));
    let f = parse_chunks([top.as_bytes()])
        .expect_err("top")
        .into_failure();
    assert_eq!(
        f.reason,
        "Validation Failed: 1: moving_fn aggregation [p] must have a histogram, date_histogram or auto_date_histogram as parent but doesn't have a parent;"
    );
    let terms = request("terms", &pipe("terms", "p", "moving_fn", body));
    let f = parse_chunks([terms.as_bytes()])
        .expect_err("terms")
        .into_failure();
    assert!(
        f.reason
            .contains("must have a histogram, date_histogram or auto_date_histogram as parent"),
        "{}",
        f.reason
    );
    for body in [
        r#""buckets_path":"a>s","window":2,"script":"1""#,
        r#""buckets_path":"_key","window":2,"script":"1""#,
        r#""buckets_path":"s","window":2,"script":{"source":"1","lang":"expression"}"#,
        r#""buckets_path":"s","window":2,"script":"1","format":"0.0""#,
    ] {
        let f = parse_with(body).expect_err("refused");
        assert!(
            f.reason.contains("not supported by this node yet"),
            "{body}: {}",
            f.reason
        );
    }
}

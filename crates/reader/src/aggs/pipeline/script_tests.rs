//! `bucket_script` and `bucket_selector` parsed and computed against the
//! recorded Java (`fixtures/pipeline-script`), their scripts the Painless
//! equivalents of the recorder's test engine's.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::parent_parse_tests::{java_chain, kind_of, pipe, request};
use super::parent_tests::{applied, histogram};
use super::script::ScriptPipe;
use super::{BucketValue, PipelineAgg, PipelineKind};
use crate::dsl::parse_chunks;
use lucene_painless::ir::Constant;

const RECORDED: &str =
    include_str!("../../../../../fixtures/pipeline-script/java-script-3.10.0.txt");

/// The recorder's test script `code` in Painless.
fn painless(code: &str) -> &'static str {
    match code {
        "sum" => "return params.x + params.y",
        "ratio" => "return params.x / params.y",
        "scaled" => "return params.x * params.k",
        "int" => "return 7",
        "long" => "return 5000000000L",
        "nan" => "return 0.0 / 0.0",
        "null" => "return null",
        "x" => "return params.x",
        "gt10" => "return params.x > 10",
        "true" => "return true",
        "false" => "return false",
        "countgt3" => "return params.y > 3",
        other => panic!("{other}"),
    }
}

fn value(path: &str) -> BucketValue {
    match path {
        "_count" => BucketValue::Count,
        other => match other.split_once('.') {
            Some((name, metric)) => BucketValue::Sub {
                name: name.to_owned(),
                metric: metric.to_owned(),
            },
            None => BucketValue::Sub {
                name: other.to_owned(),
                metric: "value".to_owned(),
            },
        },
    }
}

fn scripted(kind: PipelineKind, code: &str, paths: &[(&str, &str)], zeros: bool) -> PipelineAgg {
    let mut agg = super::parent_tests::parent("p", kind, "", zeros);
    agg.script = Some(Box::new(ScriptPipe {
        source: painless(code).to_owned(),
        params: vec![("k".to_owned(), Constant::Int(3))],
        vars: paths
            .iter()
            .map(|(n, p)| ((*n).to_owned(), value(p)))
            .collect(),
        window: 0,
        shift: 0,
    }));
    agg
}

fn recorded(name: &str) -> String {
    let line = RECORDED
        .lines()
        .find(|l| l.starts_with(&format!("{name} ")))
        .unwrap_or_else(|| panic!("no fixture {name}"));
    let json = line.split_once(' ').expect("json").1;
    json.strip_prefix(r#"{"h":"#)
        .and_then(|j| j.strip_suffix('}'))
        .expect("body")
        .to_owned()
}

// AC-1
#[test]
fn both_pipelines_give_the_bytes_java_wrote_for_each_gap_policy() {
    for gap in [false, true] {
        let g = if gap { "gap." } else { "" };
        for (policy, zeros) in [("insert_zeros", true), ("skip", false)] {
            for path in ["s", "a", "_count"] {
                let sum = scripted(
                    PipelineKind::BucketScript,
                    "sum",
                    &[("x", path), ("y", "_count")],
                    zeros,
                );
                let name = format!("{g}script.sum.{path}.{policy}");
                assert_eq!(
                    applied(&[&sum], &histogram(gap, false), false),
                    recorded(&name),
                    "{name}"
                );
                let pick = scripted(PipelineKind::BucketSelector, "gt10", &[("x", path)], zeros);
                let name = format!("{g}select.gt10.{path}.{policy}");
                assert_eq!(
                    applied(&[&pick], &histogram(gap, false), false),
                    recorded(&name),
                    "{name}"
                );
            }
            let sum = scripted(
                PipelineKind::BucketScript,
                "sum",
                &[("x", "s"), ("y", "_count")],
                zeros,
            );
            let name = format!("empty.script.sum.s.{policy}");
            assert_eq!(
                applied(&[&sum], &histogram(false, true), false),
                recorded(&name),
                "{name}"
            );
            let pick = scripted(PipelineKind::BucketSelector, "gt10", &[("x", "s")], zeros);
            let name = format!("empty.select.gt10.s.{policy}");
            assert_eq!(
                applied(&[&pick], &histogram(false, true), false),
                recorded(&name),
                "{name}"
            );
        }
    }
}

// AC-2
#[test]
fn a_script_result_is_written_as_java_writes_it() {
    let two = [("x", "s"), ("y", "_count")];
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, PipelineKind, &str, Vec<(&str, &str)>, bool, bool)> = vec![
        (
            "script.ratio",
            PipelineKind::BucketScript,
            "ratio",
            two.to_vec(),
            false,
            false,
        ),
        (
            "script.ratio.empty",
            PipelineKind::BucketScript,
            "ratio",
            two.to_vec(),
            true,
            true,
        ),
        (
            "script.params",
            PipelineKind::BucketScript,
            "scaled",
            vec![("x", "s")],
            false,
            false,
        ),
        (
            "script.int",
            PipelineKind::BucketScript,
            "int",
            vec![("x", "s")],
            false,
            false,
        ),
        (
            "script.long",
            PipelineKind::BucketScript,
            "long",
            vec![("x", "s")],
            false,
            false,
        ),
        (
            "script.nan",
            PipelineKind::BucketScript,
            "nan",
            vec![("x", "s")],
            false,
            false,
        ),
        (
            "script.null",
            PipelineKind::BucketScript,
            "null",
            vec![("x", "s")],
            false,
            false,
        ),
        (
            "script.nopath",
            PipelineKind::BucketScript,
            "int",
            vec![],
            false,
            false,
        ),
        (
            "script.badpath",
            PipelineKind::BucketScript,
            "x",
            vec![("x", "zz")],
            false,
            false,
        ),
        (
            "script.metric",
            PipelineKind::BucketScript,
            "x",
            vec![("x", "s.value")],
            false,
            false,
        ),
        (
            "script.avgmetric",
            PipelineKind::BucketScript,
            "x",
            vec![("x", "a.value")],
            false,
            false,
        ),
        (
            "select.true",
            PipelineKind::BucketSelector,
            "true",
            vec![("x", "s")],
            false,
            false,
        ),
        (
            "select.false",
            PipelineKind::BucketSelector,
            "false",
            vec![("x", "s")],
            false,
            false,
        ),
        (
            "select.count",
            PipelineKind::BucketSelector,
            "countgt3",
            vec![("y", "_count")],
            false,
            false,
        ),
        (
            "select.nopath",
            PipelineKind::BucketSelector,
            "true",
            vec![],
            false,
            false,
        ),
    ];
    for (name, kind, code, paths, zeros, empty) in cases {
        let p = scripted(kind, code, &paths, zeros);
        assert_eq!(
            applied(&[&p], &histogram(false, empty), false),
            recorded(name),
            "{name}"
        );
    }
}

// AC-3
#[test]
fn a_typed_result_is_a_simple_value() {
    let p = scripted(PipelineKind::BucketScript, "int", &[], false);
    let out = applied(&[&p], &histogram(false, false), true);
    assert!(out.contains(r#""simple_value#p":{"value":7.0}"#), "{out}");
}

// AC-4
#[test]
fn a_script_that_fails_or_returns_the_wrong_type_fails_the_search() {
    let broken = |kind, code: &str| {
        let mut p = scripted(kind, "int", &[], false);
        p.script.as_mut().expect("script").source = code.to_owned();
        p
    };
    for p in [
        broken(PipelineKind::BucketScript, "return 'a'"),
        broken(PipelineKind::BucketScript, "int x = 'a';"),
        broken(PipelineKind::BucketSelector, "return 1"),
    ] {
        let body = histogram(false, false);
        let e = super::parent::apply(body.as_bytes(), &[&p], false).expect_err("fails");
        assert_eq!(
            super::script::failure_of(&e).map(|f| f.name.as_str()),
            Some("p")
        );
    }
}

/// `{"p":{...}}` for the pipeline in a histogram request, parsed with the
/// switch on for this thread.
fn parse_with(ty: &str, body: &str) -> Result<Vec<PipelineAgg>, crate::failure::Failure> {
    crate::painless_switch::set_for_this_thread(true);
    let req = request("h", &pipe("h", "p", ty, body));
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
        let (ty, rest) = rest.split_once(' ').expect("type");
        let (body, outcome) = rest.split_once(" -> ").expect("outcome");
        let body = body
            .strip_prefix('{')
            .and_then(|b| b.strip_suffix('}'))
            .expect("object");
        let got = parse_with(ty, body);
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
        // Our script object parser words its own messages.
        if body.contains(r#""params":1"#) || !chain.iter().all(|(c, _)| kind_of(c).is_some()) {
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
    assert!(checked > 15, "{checked}");
}

// AC-6
#[test]
fn a_parsed_pipeline_keeps_its_script_variables_and_host() {
    let pipes = parse_with(
        "bucket_script",
        r#""buckets_path":{"x":"s","y":"_count"},"script":{"source":"params.x","params":{"k":1}},"gap_policy":"insert_zeros""#,
    )
    .expect("parses");
    let p = &pipes[0];
    assert_eq!(p.kind, PipelineKind::BucketScript);
    assert_eq!(p.host.as_deref(), Some("h"));
    assert!(p.insert_zeros);
    let s = p.script.as_ref().expect("script");
    assert_eq!(s.source, "params.x");
    assert_eq!(s.params, vec![("k".to_owned(), Constant::Int(1))]);
    assert_eq!(s.vars.len(), 2);
}

// AC-7
#[test]
fn the_switch_gates_both_and_the_top_level_refuses_them() {
    crate::painless_switch::set_for_this_thread(false);
    let off = request(
        "h",
        &pipe(
            "h",
            "p",
            "bucket_script",
            r#""buckets_path":"s","script":"1""#,
        ),
    );
    let f = parse_chunks([off.as_bytes()])
        .expect_err("off")
        .into_failure();
    assert!(
        f.reason.contains("not supported by this node yet"),
        "{}",
        f.reason
    );
    crate::painless_switch::set_for_this_thread(true);
    for ty in ["bucket_script", "bucket_selector"] {
        let top = request(
            "top",
            &pipe("top", "p", ty, r#""buckets_path":"s","script":"1""#),
        );
        let f = parse_chunks([top.as_bytes()])
            .expect_err("top")
            .into_failure();
        assert_eq!(
            f.reason,
            format!(
                "Validation Failed: 1: {ty} aggregation [p] must be declared inside of another aggregation;"
            )
        );
    }
    for body in [
        r#""buckets_path":{"x":"a>s"},"script":"1""#,
        r#""buckets_path":{"x":"_key"},"script":"1""#,
        r#""buckets_path":{"x":"s"},"script":{"source":"1","lang":"expression"}"#,
        r#""buckets_path":{"x":"s"},"script":"1","format":"0.0""#,
    ] {
        let f = parse_with("bucket_script", body).expect_err("refused");
        assert!(
            f.reason.contains("not supported by this node yet"),
            "{body}: {}",
            f.reason
        );
    }
}

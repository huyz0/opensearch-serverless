//! Sibling pipeline aggregations parsed, validated and computed against the recorded Java.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::compute::write_result;
use super::parse::validate;
use super::{BucketValue, PipelineAgg, PipelineKind};
use crate::dsl::parse_chunks;

const RECORDED: &str =
    include_str!("../../../../../fixtures/pipeline-aggs/java-pipeline-3.10.0.txt");
const VALIDATION: &str =
    include_str!("../../../../../fixtures/pipeline-aggs/java-validation-3.10.0.txt");

/// The rendered body of the recorder's `terms` over four buckets, the second
/// with an `avg` that has no value.
const BUCKETS: &str = r#"{"doc_count_error_upper_bound":0,"sum_other_doc_count":0,"buckets":[{"key":1,"doc_count":5,"a":{"value":10.0}},{"key":2,"doc_count":3,"a":{"value":null}},{"key":3,"doc_count":4,"a":{"value":4.0}},{"key":4,"doc_count":1,"a":{"value":-2.0}}]}"#;
const TIES: &str = r#"{"buckets":[{"key":1,"doc_count":2,"a":{"value":3.0}},{"key":2,"doc_count":2,"a":{"value":3.0}},{"key":3,"doc_count":7,"a":{"value":3.0}}]}"#;
const EMPTY: &str = r#"{"buckets":[]}"#;

fn recorded(name: &str) -> String {
    let line = RECORDED
        .lines()
        .find(|l| l.starts_with(&format!("{name} ")))
        .unwrap_or_else(|| panic!("no fixture {name}"));
    // `name type {"p":{BODY}}`
    let json = line.splitn(3, ' ').nth(2).expect("json");
    json.strip_prefix(r#"{"p":{"#)
        .and_then(|j| j.strip_suffix("}}"))
        .expect("a body")
        .to_owned()
}

fn pipeline(kind: PipelineKind, count: bool, insert_zeros: bool) -> PipelineAgg {
    PipelineAgg {
        name: "p".to_owned(),
        kind,
        path: String::new(),
        agg: "t".to_owned(),
        value: if count {
            BucketValue::Count
        } else {
            BucketValue::Sub {
                name: "a".to_owned(),
                metric: "value".to_owned(),
            }
        },
        insert_zeros,
        host: None,
        lag: 1,
        sort: None,
        script: None,
    }
}

fn computed(p: &PipelineAgg, input: &str) -> String {
    let mut out = Vec::new();
    write_result(&mut out, p, input.as_bytes()).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

// AC-1, AC-2, AC-3
#[test]
fn every_kind_gives_the_bytes_java_wrote_for_both_gap_policies_and_paths() {
    for (tag, kind) in [
        ("avg", PipelineKind::Avg),
        ("sum", PipelineKind::Sum),
        ("min", PipelineKind::Min),
        ("max", PipelineKind::Max),
        ("stats", PipelineKind::Stats),
    ] {
        for (path, count) in [("a", false), ("count", true)] {
            for (gap, zeros) in [("skip", false), ("insert_zeros", true)] {
                let name = format!("{tag}.{path}.{gap}");
                assert_eq!(
                    computed(&pipeline(kind, count, zeros), BUCKETS),
                    recorded(&name),
                    "{name}"
                );
            }
        }
    }
}

// AC-4
#[test]
fn ties_list_every_key_and_no_buckets_give_null_or_zero() {
    let p = |kind| pipeline(kind, false, false);
    assert_eq!(computed(&p(PipelineKind::Min), TIES), recorded("min.ties"));
    assert_eq!(computed(&p(PipelineKind::Max), TIES), recorded("max.ties"));
    for (tag, kind) in [
        ("avg", PipelineKind::Avg),
        ("sum", PipelineKind::Sum),
        ("min", PipelineKind::Min),
        ("max", PipelineKind::Max),
        ("stats", PipelineKind::Stats),
    ] {
        let name = format!("{tag}.empty");
        assert_eq!(
            computed(&pipeline(kind, true, false), EMPTY),
            recorded(&name),
            "{name}"
        );
    }
}

// AC-5
#[test]
fn keyed_buckets_and_date_keys_are_read_as_java_reads_them() {
    let keyed = r#"{"buckets":{"lo":{"doc_count":2},"hi":{"doc_count":9}}}"#;
    let max = pipeline(PipelineKind::Max, true, false);
    assert_eq!(computed(&max, keyed), r#""value":9.0,"keys":["hi"]"#);
    let dated = r#"{"buckets":[{"key_as_string":"2020-01-01T00:00:00.000Z","key":1577836800000,"doc_count":4},{"key":2.5,"doc_count":1},{"key":"x","doc_count":8}]}"#;
    let min = pipeline(PipelineKind::Min, true, false);
    assert_eq!(computed(&min, dated), r#""value":1.0,"keys":["2.5"]"#);
    let max = pipeline(PipelineKind::Max, true, false);
    assert_eq!(computed(&max, dated), r#""value":8.0,"keys":["x"]"#);
    let stat = r#"{"buckets":[{"key":1,"doc_count":1,"s":{"max":5.0}},{"key":2,"doc_count":1,"s":{"max":7.0}}]}"#;
    let by_metric = PipelineAgg {
        value: BucketValue::Sub {
            name: "s".to_owned(),
            metric: "max".to_owned(),
        },
        ..pipeline(PipelineKind::Sum, false, false)
    };
    assert_eq!(computed(&by_metric, stat), r#""value":12.0"#);
}

fn aggs_and_pipes(body: &str) -> Result<(), String> {
    let source = parse_chunks([body.as_bytes()]).map_err(|e| {
        let f = e.into_failure();
        f.reason.clone()
    })?;
    assert!(!source.pipelines.is_empty());
    Ok(())
}

fn request(path: &str) -> String {
    format!(
        r#"{{"aggs":{{"t":{{"terms":{{"field":"f"}},"aggs":{{"a":{{"avg":{{"field":"n"}}}}}}}},"m":{{"avg":{{"field":"n"}}}},"p":{{"avg_bucket":{{"buckets_path":"{path}"}}}}}}}}"#
    )
}

// AC-6
#[test]
fn a_path_is_validated_as_java_validates_it() {
    for line in VALIDATION.lines() {
        let (path, expected) = line.split_once(' ').unwrap_or((line, ""));
        let got = aggs_and_pipes(&request(path));
        match (expected, &got) {
            // Paths this node refuses rather than resolves.
            ("ok", Err(reason)) => assert!(
                reason.contains("not supported by this node yet"),
                "{path}: {reason}"
            ),
            ("ok", Ok(())) => {}
            (expected, Err(reason)) if !expected.is_empty() && !path.is_empty() => {
                // The recorder joins messages with ` | `; one is expected.
                assert_eq!(
                    reason,
                    &format!("Validation Failed: 1: {expected};"),
                    "{path}"
                );
            }
            _ => {}
        }
    }
}

// AC-7
#[test]
fn a_sibling_pipeline_beside_the_aggregation_it_reads_parses() {
    let source = parse_chunks([request("t>a").as_bytes()]).expect("parses");
    assert_eq!(source.aggs.len(), 2);
    assert_eq!(source.pipelines.len(), 1);
    let p = &source.pipelines[0];
    assert_eq!(
        (p.kind, p.agg.as_str(), p.insert_zeros),
        (PipelineKind::Avg, "t", false)
    );
    let zeros = r#"{"aggs":{"t":{"terms":{"field":"f"}},"p":{"sum_bucket":{"buckets_path":["t>_count"],"gap_policy":"insert_zeros"}}}}"#;
    let source = parse_chunks([zeros.as_bytes()]).expect("parses");
    let p = &source.pipelines[0];
    assert_eq!(
        (p.kind, p.value.clone(), p.insert_zeros),
        (PipelineKind::Sum, BucketValue::Count, true)
    );
}

// AC-8
#[test]
fn bad_bodies_fail_as_java_fails() {
    let body = |inner: &str| {
        format!(
            r#"{{"aggs":{{"t":{{"terms":{{"field":"f"}}}},"p":{{"avg_bucket":{{{inner}}}}}}}}}"#
        )
    };
    let reason = |inner: &str| {
        parse_chunks([body(inner).as_bytes()])
            .expect_err("refused")
            .into_failure()
            .reason
            .clone()
    };
    assert_eq!(
        reason(""),
        "Missing required field [buckets_path] for aggregation [p]"
    );
    assert_eq!(
        reason(r#""buckets_path":1"#),
        "Unexpected token VALUE_NUMBER [buckets_path] in [p]"
    );
    assert_eq!(
        reason(r#""buckets_path":"t>_count","gap_policy":"nope""#),
        "Invalid gap policy: [nope], accepted values: [insert_zeros, skip]"
    );
    assert_eq!(
        reason(r#""buckets_path":"t>_count","gap_policy":"SKIP""#),
        "Invalid gap policy: [SKIP], accepted values: [insert_zeros, skip]"
    );
    assert_eq!(
        reason(r#""buckets_path":"t>_count","foo":1"#),
        "Unexpected token VALUE_NUMBER [foo] in [p]"
    );
    assert!(
        reason(r#""buckets_path":"t>_count","format":"0.0""#)
            .contains("not supported by this node yet")
    );
    // Under another aggregation, or over a path of more than two elements.
    let nested = r#"{"aggs":{"t":{"terms":{"field":"f"},"aggs":{"p":{"avg_bucket":{"buckets_path":"a>b"}}}}}}"#;
    let failure = parse_chunks([nested.as_bytes()])
        .expect_err("refused")
        .into_failure();
    assert!(
        failure.reason.contains("not supported by this node yet"),
        "{}",
        failure.reason
    );
    let validated = validate(&[], &[]);
    assert!(validated.is_ok());
}

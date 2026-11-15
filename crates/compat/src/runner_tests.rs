use std::collections::{BTreeMap, VecDeque};

use serde_json::{Value, json};

use super::*;
use crate::fixture::{Request, Response};

/// A target that answers with canned responses in order and records what it
/// was sent.
struct FakeTarget {
    answers: VecDeque<Result<Response, TargetError>>,
    sent: Vec<String>,
}

impl FakeTarget {
    fn new(answers: Vec<Result<Response, TargetError>>) -> Self {
        Self {
            answers: answers.into(),
            sent: Vec::new(),
        }
    }
}

impl Target for FakeTarget {
    fn send(&mut self, request: &Request) -> Result<Response, TargetError> {
        self.sent
            .push(format!("{} {}", request.method.as_str(), request.path));
        self.answers.pop_front().unwrap_or_else(|| {
            Err(TargetError {
                message: "no canned answer".into(),
            })
        })
    }
}

fn response(status: u16, content_type: &str, body: Value) -> Response {
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_owned(), content_type.to_owned());
    headers.insert("x-opaque-id".to_owned(), "differs every run".to_owned());
    Response {
        status,
        headers,
        body: Some(body),
    }
}

/// A recorded search fixture with cleanup, one step, `took` normalised, and
/// only `content-type` selected.
fn search_fixture() -> Fixture {
    Fixture::from_value(&json!({
        "name": "match_all",
        "cleanup": [{"method": "DELETE", "path": "/compat-search"}],
        "steps": [{
            "name": "search",
            "request": {"method": "POST", "path": "/compat-search/_search", "body": {"query": {"match_all": {}}}},
            "response": {
                "status": 200,
                "headers": {"content-type": "application/json; charset=UTF-8"},
                "body": {"took": 4, "timed_out": false, "hits": {"total": {"value": 2, "relation": "eq"}}}
            },
            "normalise": [{"pointer": "/body/took", "reason": "elapsed time"}]
        }]
    }))
    .unwrap()
}

fn search_body(took: u64, total: u64) -> Value {
    json!({"took": took, "timed_out": false, "hits": {"total": {"value": total, "relation": "eq"}}})
}

const JSON: &str = "application/json; charset=UTF-8";

#[test]
fn a_difference_only_in_a_normalised_field_passes() {
    let mut target = FakeTarget::new(vec![
        Ok(response(404, JSON, json!({}))),
        Ok(response(200, JSON, search_body(17, 2))),
    ]);
    let report = run_fixture(&mut target, &search_fixture());
    assert!(report.passed(), "{report}");
    assert_eq!(
        target.sent,
        ["DELETE /compat-search", "POST /compat-search/_search"]
    );
    assert_eq!(report.to_string(), "match_all: ok\n  search: ok\n");
}

#[test]
fn a_real_body_difference_fails_and_names_its_pointer() {
    let mut target = FakeTarget::new(vec![
        Ok(response(200, JSON, json!({}))),
        Ok(response(200, JSON, search_body(17, 3))),
    ]);
    let report = run_fixture(&mut target, &search_fixture());
    assert!(!report.passed());
    let StepOutcome::Failed {
        differences,
        stale_rules,
    } = &report.steps[0].outcome
    else {
        panic!("expected a failure, got {report}");
    };
    assert!(stale_rules.is_empty());
    assert_eq!(differences.listed.len(), 1);
    assert_eq!(
        differences.listed[0].to_string(),
        "/body/hits/total/value: expected 2, got 3"
    );
    let text = report.to_string();
    assert!(
        text.starts_with("match_all: FAILED\n  search: differs\n"),
        "{text}"
    );
    assert!(
        text.contains("    /body/hits/total/value: expected 2, got 3\n"),
        "{text}"
    );
}

#[test]
fn status_and_selected_headers_are_compared_and_others_ignored() {
    let mut target = FakeTarget::new(vec![
        Ok(response(200, JSON, json!({}))),
        Ok(response(201, "text/plain", search_body(1, 2))),
    ]);
    let report = run_fixture(&mut target, &search_fixture());
    let StepOutcome::Failed { differences, .. } = &report.steps[0].outcome else {
        panic!("expected a failure, got {report}");
    };
    let lines: Vec<String> = differences.listed.iter().map(ToString::to_string).collect();
    assert_eq!(
        lines,
        [
            r#"/headers/content-type: expected "application/json; charset=UTF-8", got "text/plain""#,
            "/status: expected 200, got 201",
        ],
        "x-opaque-id is not selected by the fixture, so it is not compared"
    );
}

#[test]
fn a_target_error_is_reported_and_the_suite_goes_on() {
    let mut target = FakeTarget::new(vec![
        Ok(response(200, JSON, json!({}))),
        Err(TargetError {
            message: "connection refused".into(),
        }),
        Ok(response(200, JSON, json!({}))),
        Ok(response(200, JSON, search_body(2, 2))),
    ]);
    let fixtures = [search_fixture(), search_fixture()];
    let report = run_suite(&mut target, &fixtures);
    assert!(!report.passed());
    assert_eq!(
        report.fixtures[0].steps[0].outcome,
        StepOutcome::TargetFailed(TargetError {
            message: "connection refused".into()
        })
    );
    assert!(report.fixtures[1].passed(), "the second fixture still ran");
    let text = report.to_string();
    assert!(
        text.contains("  search: target error: connection refused\n"),
        "{text}"
    );
    assert!(text.ends_with("2 fixtures, 1 failed\n"), "{text}");
}

#[test]
fn a_stale_rule_fails_the_step_even_when_bodies_match() {
    let mut fixture = search_fixture();
    fixture.steps[0].normalise.push(crate::fixture::Rule {
        pointer: crate::pointer::Pointer::parse("/body/_shards").unwrap(),
        reason: "shard counts vary".into(),
    });
    let mut target = FakeTarget::new(vec![
        Ok(response(200, JSON, json!({}))),
        Ok(response(200, JSON, search_body(9, 2))),
    ]);
    let report = run_fixture(&mut target, &fixture);
    let StepOutcome::Failed {
        differences,
        stale_rules,
    } = &report.steps[0].outcome
    else {
        panic!("expected a failure, got {report}");
    };
    assert!(differences.is_empty());
    assert_eq!(stale_rules, &["/body/_shards"]);
    assert!(report.to_string().contains("stale rule /body/_shards"));
}

#[test]
fn an_unrecorded_step_is_reported_as_such() {
    let mut fixture = search_fixture();
    fixture.steps[0].response = None;
    let mut target = FakeTarget::new(vec![
        Ok(response(200, JSON, json!({}))),
        Ok(response(200, JSON, search_body(9, 2))),
    ]);
    let report = run_fixture(&mut target, &fixture);
    assert_eq!(report.steps[0].outcome, StepOutcome::NotRecorded);
    assert!(!report.passed());
    assert!(report.to_string().contains("search: not recorded"));
}

#[test]
fn an_empty_suite_passes() {
    let report = run_suite(&mut FakeTarget::new(Vec::new()), &[]);
    assert!(report.passed());
    assert_eq!(report.to_string(), "0 fixtures, 0 failed\n");
}

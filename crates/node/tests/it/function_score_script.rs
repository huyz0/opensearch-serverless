//! docs/specs/function-score-script: `script_score` in the expression
//! language, end to end over the compat blobs.

use crate::support::{counts, request, start};

fn search(node: &crate::support::Node, function: &str) -> (u16, String) {
    let body = format!(
        r#"{{"size":10,"query":{{"function_score":{{"query":{{"match_all":{{}}}},"boost_mode":"replace",{function}}}}}}}"#
    );
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

/// The `_id` and `_score` of each hit, in order.
fn scores(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find(r#""_id":""#) {
        rest = &rest[i + 7..];
        let id = rest[..rest.find('"').expect("id")].to_owned();
        let j = rest.find(r#""_score":"#).expect("score") + 9;
        let end = rest[j..].find([',', '}']).expect("end") + j;
        out.push((id, rest[j..end].to_owned()));
    }
    out
}

fn sorted(mut v: Vec<(String, String)>) -> Vec<(String, String)> {
    v.sort();
    v
}

const SCRIPT: &str = r#""script_score":{"script":{"lang":"expression","source":"doc['year'].value * factor","params":{"factor":2}}}"#;

// AC-1
#[test]
fn an_expression_scores_like_the_same_arithmetic() {
    let node = start();
    let (status, scripted) = search(&node, SCRIPT);
    assert_eq!(status, 200, "{scripted}");
    let (status, plain) = search(&node, r#""field_value_factor":{"field":"year","factor":2}"#);
    assert_eq!(status, 200, "{plain}");
    let got = scores(&scripted);
    assert!(!got.is_empty(), "{scripted}");
    assert_eq!(sorted(got), sorted(scores(&plain)));
}

// AC-2
#[test]
fn a_script_takes_a_filter_and_a_weight() {
    let node = start();
    let function = r#""functions":[{"filter":{"range":{"year":{"gte":2002}}},"weight":3,"script_score":{"script":{"lang":"expression","source":"doc['year'].value","params":{}}}}],"score_mode":"sum""#;
    let (status, body) = search(&node, function);
    assert_eq!(status, 200, "{body}");
    let got = scores(&body);
    assert!(got.iter().any(|(_, s)| s.starts_with("6")), "{body}");
}

// AC-3
#[test]
fn what_is_not_served_is_refused() {
    let node = start();
    for (function, reason) in [
        (
            r#""script_score":{"script":"1"}"#,
            "runs only lang [expression]",
        ),
        (
            r#""script_score":{"script":{"lang":"painless","source":"1"}}"#,
            "runs only lang [expression]",
        ),
        (
            r#""script_score":{"script":{"id":"s"}}"#,
            "a stored script [id]",
        ),
        (
            r#""script_score":{"script":{"lang":"expression","source":"doc['title'].value"}}"#,
            "not a number or a date field",
        ),
        (
            r#""script_score":{"script":{"lang":"expression","source":"nope"}}"#,
            "Variable [nope] is not a number param",
        ),
        (
            r#""script_score":{"script":{"lang":"expression","source":"doc['year'].size()"}}"#,
            "not served",
        ),
    ] {
        let (status, body) = search(&node, function);
        assert_eq!(status, 400, "{function}: {body}");
        assert!(body.contains(reason), "{function}: {body}");
    }
}

// AC-4
#[test]
fn a_script_adds_no_store_request() {
    let node = start();
    let plain = r#""field_value_factor":{"field":"year","factor":2}"#;
    search(&node, plain);
    let before = counts(&node.store);
    search(&node, plain);
    let plain_cost = counts(&node.store)
        .iter()
        .zip(before.iter())
        .map(|(a, b)| a - b)
        .collect::<Vec<_>>();
    let before = counts(&node.store);
    search(&node, SCRIPT);
    let scripted_cost = counts(&node.store)
        .iter()
        .zip(before.iter())
        .map(|(a, b)| a - b)
        .collect::<Vec<_>>();
    assert_eq!(scripted_cost, plain_cost);
}

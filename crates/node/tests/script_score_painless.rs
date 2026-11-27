//! docs/specs/painless-script-score: `script_score` in Painless, compiled by
//! the node, end to end over the compat blobs. A process of its own, because
//! the node's switch is process-wide.

// ARITH: test sizes and counts are small constants; an overflow panics
// the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "support/mod.rs"]
mod support;

use support::{Node, counts, request, start};

fn search(node: &Node, function: &str) -> (u16, String) {
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

/// The `_id` and `_score` of each hit, sorted.
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
    out.sort();
    out
}

fn on() {
    oss_reader::painless_switch::set_native_compile(true);
}

const PAINLESS: &str = r#""script_score":{"script":{"source":"doc['year'].value * params.factor","params":{"factor":2}}}"#;
const EXPRESSION: &str = r#""script_score":{"script":{"lang":"expression","source":"doc['year'].value * factor","params":{"factor":2}}}"#;

// AC-2
#[test]
fn painless_scores_like_the_same_expression() {
    on();
    let node = start();
    let (status, painless) = search(&node, PAINLESS);
    assert_eq!(status, 200, "{painless}");
    let (status, expression) = search(&node, EXPRESSION);
    assert_eq!(status, 200, "{expression}");
    let got = scores(&painless);
    assert!(!got.is_empty(), "{painless}");
    assert_eq!(got, scores(&expression));
}

// AC-3
#[test]
fn a_script_reads_the_score_and_declares_variables() {
    on();
    let node = start();
    let function = r#""script_score":{"script":{"lang":"painless","source":"double y = doc['year'].value; return y + _score;"}}"#;
    let (status, body) = search(&node, function);
    assert_eq!(status, 200, "{body}");
    // Years 1999 to 2010 and a match_all score of 1.
    assert!(
        scores(&body).iter().any(|(_, s)| s.starts_with("2000")),
        "{body}"
    );
}

// AC-4
#[test]
fn what_painless_rejects_or_this_node_cannot_run_is_refused() {
    on();
    let node = start();
    for (source, reason) in [
        ("int x = 'a'; return x;", "compile error"),
        ("doc['nope'].value", "No field found for [nope] in mapping"),
        ("doc['title'].value", "script_exception"),
    ] {
        let function =
            format!(r#""script_score":{{"script":{{"lang":"painless","source":"{source}"}}}}"#);
        let (status, body) = search(&node, &function);
        assert_eq!(status, 400, "{source}: {body}");
        assert!(body.contains(reason), "{source}: {body}");
    }
    let (status, body) = search(&node, r#""script_score":{"script":{"id":"s"}}"#);
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("a stored script [id]"), "{body}");
}

// AC-5
#[test]
fn a_painless_script_adds_no_store_request() {
    on();
    let node = start();
    search(&node, EXPRESSION);
    let before = counts(&node.store);
    search(&node, EXPRESSION);
    let expression: Vec<u64> = counts(&node.store)
        .iter()
        .zip(before.iter())
        .map(|(a, b)| a - b)
        .collect();
    let before = counts(&node.store);
    search(&node, PAINLESS);
    let painless: Vec<u64> = counts(&node.store)
        .iter()
        .zip(before.iter())
        .map(|(a, b)| a - b)
        .collect();
    assert_eq!(painless, expression);
}

fn search_with(node: &Node, extra: &str) -> (u16, String) {
    let body =
        format!(r#"{{"size":10,"query":{{"match_all":{{}}}},"sort":[{{"year":"asc"}}],{extra}}}"#);
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

// AC-6
#[test]
fn a_painless_script_field_is_written_for_each_hit() {
    on();
    let node = start();
    let fields = r#""script_fields":{"doubled":{"script":{"source":"doc['year'].value * params.k","params":{"k":2}}}}"#;
    let (status, body) = search_with(&node, fields);
    assert_eq!(status, 200, "{body}");
    for want in ["3998.0", "4002.0", "4004.0", "4020.0"] {
        assert!(
            body.contains(&format!(r#""fields":{{"doubled":[{want}]}}"#)),
            "{want}: {body}"
        );
    }
    let reads_score = r#""script_fields":{"a":{"script":{"source":"_score"}}}"#;
    let (status, body) = search_with(&node, reads_score);
    assert_eq!(status, 400, "{body}");
}

// AC-7
#[test]
fn a_painless_script_sorts_hits() {
    on();
    let node = start();
    let body = r#"{"size":10,"query":{"match_all":{}},"sort":[{"_script":{"type":"number","order":"desc","script":{"source":"doc['year'].value * params.k","params":{"k":2}}}}]}"#;
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    let text = r.text();
    assert_eq!(r.status, 200, "{text}");
    // Years 2010, 2002, 2002, 2001, 1999, doubled and written as doubles.
    let want = ["4020.0", "4004.0", "4004.0", "4002.0", "3998.0"];
    let mut rest = text.as_str();
    for w in want {
        let i = rest.find(r#""sort":["#).expect("sort") + 8;
        assert!(rest[i..].starts_with(w), "{w}: {text}");
        rest = &rest[i..];
    }
}

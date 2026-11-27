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

fn query_search(node: &Node, query: &str) -> (u16, String) {
    let body = format!(r#"{{"size":10,"query":{query},"sort":[{{"year":"asc"}}]}}"#);
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

fn ids(body: &str) -> Vec<String> {
    scores(body).into_iter().map(|(id, _)| id).collect()
}

// AC-8
#[test]
fn a_script_query_matches_where_the_script_is_true() {
    on();
    let node = start();
    let (status, all) = query_search(&node, r#"{"match_all":{}}"#);
    assert_eq!(status, 200, "{all}");
    let (status, scripted) = query_search(
        &node,
        r#"{"script":{"script":{"source":"doc['year'].value >= params.y","params":{"y":2002}}}}"#,
    );
    assert_eq!(status, 200, "{scripted}");
    let (status, ranged) = query_search(&node, r#"{"range":{"year":{"gte":2002}}}"#);
    assert_eq!(status, 200, "{ranged}");
    assert!(ids(&scripted).len() < ids(&all).len(), "{scripted}");
    assert_eq!(ids(&scripted), ids(&ranged), "{scripted} {ranged}");
    // A filter inside a bool matches the same hits.
    let (status, filtered) = query_search(
        &node,
        r#"{"bool":{"filter":[{"script":{"script":"doc['year'].value >= 2002"}}]}}"#,
    );
    assert_eq!(status, 200, "{filtered}");
    assert_eq!(ids(&filtered), ids(&ranged), "{filtered}");
    // A script that does not return a boolean is Painless's own rejection.
    let (status, body) = query_search(&node, r#"{"script":{"script":"1 + 1"}}"#);
    assert_eq!(status, 400, "{body}");
}

fn aggregate(node: &Node, aggs: &str) -> (u16, String) {
    let body = format!(r#"{{"size":0,"query":{{"match_all":{{}}}},"aggs":{aggs}}}"#);
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

// AC-9
#[test]
fn a_metric_aggregation_reads_a_painless_script() {
    on();
    let node = start();
    // Years 2010, 2002, 2002, 2001, 1999, doubled: sum 20,028, mean 4,005.6.
    let script = r#""script":{"source":"doc['year'].value * params.k","params":{"k":2}}"#;
    let aggs = format!(
        r#"{{"s":{{"sum":{{{script}}}}},"lo":{{"min":{{{script}}}}},"hi":{{"max":{{{script}}}}},"a":{{"avg":{{{script}}}}},"n":{{"value_count":{{{script}}}}},"st":{{"stats":{{{script}}}}}}}"#
    );
    let (status, text) = aggregate(&node, &aggs);
    assert_eq!(status, 200, "{text}");
    for want in [
        r#""s":{"value":20028.0}"#,
        r#""lo":{"value":3998.0}"#,
        r#""hi":{"value":4020.0}"#,
        r#""n":{"value":5}"#,
    ] {
        assert!(text.contains(want), "{want}: {text}");
    }
    assert!(text.contains(r#""count":5"#), "{text}");
    // the same sum as the field's, doubled
    let (_, plain) = aggregate(&node, r#"{"s":{"sum":{"field":"year"}}}"#);
    assert!(plain.contains(r#""s":{"value":10014.0}"#), "{plain}");

    // a script and a field together, a stored script and a Painless error are refused
    for (agg, reason) in [
        (
            r#"{"x":{"sum":{"field":"year","script":{"source":"1"}}}}"#,
            "not supported",
        ),
        (
            r#"{"x":{"sum":{"script":{"id":"stored"}}}}"#,
            "stored script",
        ),
        (
            r#"{"x":{"sum":{"script":{"source":"doc['year'].value *"}}}}"#,
            "compile error",
        ),
        (
            r#"{"x":{"sum":{"script":{"source":"doc['nope'].value"}}}}"#,
            "nope",
        ),
    ] {
        let (status, text) = aggregate(&node, agg);
        assert!(status >= 400, "{agg}: {text}");
        assert!(text.contains(reason), "{agg}: {text}");
    }
    // other metrics keep refusing a script
    let (status, text) = aggregate(&node, r#"{"x":{"cardinality":{"script":{"source":"1"}}}}"#);
    assert!(status >= 400 && text.contains("script"), "{text}");
}

// AC-2, AC-3, AC-4
#[test]
fn a_scripted_metric_folds_the_documents_and_reduces_the_shards() {
    on();
    let node = start();
    let agg = |reduce: &str| {
        format!(
            r#"{{"sm":{{"scripted_metric":{{"params":{{"k":1}},"init_script":"state.sum = 0L; state.n = 0","map_script":"state.sum = state.sum + doc['year'].value * params.k; state.n = state.n + 1","combine_script":"return state","reduce_script":"{reduce}"}}}}}}"#
        )
    };
    // Years 2010, 2002, 2002, 2001, 1999.
    let (status, text) = aggregate(
        &node,
        &agg("long s = 0; for (st in states) { s = s + st.sum; } return s;"),
    );
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(r#""sm":{"value":10014}"#), "{text}");
    let (status, text) = aggregate(
        &node,
        &agg(
            "def total = 0; for (st in states) { total = total + st.n; } return [total, states.size()];",
        ),
    );
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(r#""sm":{"value":[5,1]}"#), "{text}");
    // a query narrows what the map script sees
    let body = r#"{"size":0,"query":{"range":{"year":{"gte":2002}}},"aggs":{"sm":{"scripted_metric":{"map_script":"state.n = (state.n == null ? 0 : state.n) + 1","combine_script":"return state.n","reduce_script":"return states[0]"}}}}"#;
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.text().contains(r#""sm":{"value":3}"#), "{}", r.text());
}

// AC-3, AC-5
#[test]
fn a_scripted_metric_refuses_what_it_does_not_serve() {
    on();
    let node = start();
    let ok = r#""map_script":"1","combine_script":"return 1","reduce_script":"return 1""#;
    for (agg, reason) in [
        (
            r#"{"x":{"scripted_metric":{"combine_script":"return 1","reduce_script":"return 1"}}}"#.to_owned(),
            "mapScript",
        ),
        (
            r#"{"x":{"scripted_metric":{"map_script":"1","reduce_script":"return 1"}}}"#.to_owned(),
            "combineScript",
        ),
        (
            r#"{"x":{"scripted_metric":{"map_script":"1","combine_script":"return 1"}}}"#.to_owned(),
            "reduceScript",
        ),
        (
            r#"{"x":{"scripted_metric":{"map_script":{"id":"s"},"combine_script":"return 1","reduce_script":"return 1"}}}"#.to_owned(),
            "stored script",
        ),
        (
            r#"{"x":{"scripted_metric":{"map_script":{"lang":"expression","source":"1"},"combine_script":"return 1","reduce_script":"return 1"}}}"#.to_owned(),
            "painless",
        ),
        (
            format!(r#"{{"x":{{"scripted_metric":{{{ok},"bogus":1}}}}}}"#),
            "bogus",
        ),
        (
            r#"{"t":{"terms":{"field":"year"},"aggs":{"x":{"scripted_metric":{OK}}}}}"#
                .replace("OK", ok),
            "scripted_metric",
        ),
        (
            r#"{"x":{"scripted_metric":{"map_script":"state.a = doc['year'].value *","combine_script":"return 1","reduce_script":"return 1"}}}"#.to_owned(),
            "compile error",
        ),
        (
            r#"{"x":{"scripted_metric":{"map_script":"1","combine_script":"return state.nope.x","reduce_script":"return 1"}}}"#.to_owned(),
            "script_exception",
        ),
    ] {
        let (status, text) = aggregate(&node, &agg);
        assert!(status >= 400, "{agg}: {text}");
        assert!(text.contains(reason), "{agg}: {text}");
    }
}

// AC-3: a stored script is refused the same way in every context, naming no
// other context and saying what to send instead.
#[test]
fn a_stored_script_is_refused_the_same_way_in_every_context() {
    on();
    let node = start();
    let (status, text) = query_search(&node, r#"{"script":{"script":{"id":"s"}}}"#);
    assert_eq!(status, 400, "{text}");
    let (agg_status, agg) = aggregate(&node, r#"{"x":{"sum":{"script":{"id":"s"}}}}"#);
    assert_eq!(agg_status, 400, "{agg}");
    for body in [&text, &agg] {
        assert!(
            body.contains(
                "[script] a stored script [id] is not served: send the script as [source]"
            ),
            "{body}"
        );
        assert!(!body.contains("[script_score]"), "{body}");
    }
}

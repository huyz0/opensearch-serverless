//! docs/specs/msearch-api: `_msearch` end to end.

use crate::support::{Node, counts, request, start};
#[cfg(feature = "coordinator")]
use crate::support::{put_shard_copy, put_variant};

const TERM: &str = r#"{"query":{"term":{"title":"brown"}}}"#;
const ALL: &str = r#"{"query":{"match_all":{}}}"#;

fn msearch(node: &Node, path: &str, body: &str) -> (u16, String) {
    let r = request(node.addr(), "POST", path, Some(body.as_bytes()));
    (r.status, r.text())
}

/// `s` with every `"took":N` set to 0.
fn norm(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(at) = rest.find(r#""took":"#) {
        let (head, tail) = rest.split_at(at + 7);
        out.push_str(head);
        out.push('0');
        rest = tail.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    out.push_str(rest);
    out
}

/// What an item is for the search `body` of `index`.
fn item(node: &Node, index: &str, body: &str) -> String {
    let r = request(
        node.addr(),
        "POST",
        &format!("/{index}/_search"),
        Some(body.as_bytes()),
    );
    assert_eq!(r.status, 200, "{}", r.text());
    let text = r.text();
    let inner = text.strip_suffix('}').expect("an object");
    norm(&format!(r#"{inner},"status":200}}"#))
}

/// The items of an answer, and its `took`.
fn split(answer: &str) -> (String, u64) {
    let rest = answer
        .strip_prefix(r#"{"responses":["#)
        .unwrap_or_else(|| panic!("{answer}"));
    let (items, took) = rest.rsplit_once(r#"],"took":"#).expect("took");
    (
        norm(items),
        took.trim_end_matches('}').parse().expect("a number"),
    )
}

// AC-1
#[test]
fn each_item_is_its_searchs_response_with_a_status_and_took_follows() {
    let node = start();
    let body =
        format!("{{\"index\":\"compat-term\"}}\n{TERM}\n{{\"index\":\"compat-term\"}}\n{ALL}\n");
    let (status, answer) = msearch(&node, "/_msearch", &body);
    assert_eq!(status, 200, "{answer}");
    let (items, _) = split(&answer);
    assert_eq!(
        items,
        format!(
            "{},{}",
            item(&node, "compat-term", TERM),
            item(&node, "compat-term", ALL)
        )
    );
}

// AC-2
#[test]
fn the_path_index_is_the_default_and_a_header_index_overrides_it() {
    let node = start();
    let body = format!("{{}}\n{TERM}\n{{\"index\":\"compat-term\"}}\n{ALL}\n");
    let (status, answer) = msearch(&node, "/compat-term/_msearch", &body);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(
        split(&answer).0,
        format!(
            "{},{}",
            item(&node, "compat-term", TERM),
            item(&node, "compat-term", ALL)
        )
    );
    let body = format!("{{\"index\":\"compat-term\"}}\n{ALL}\n");
    let (_, answer) = msearch(&node, "/no-such-index/_msearch", &body);
    assert_eq!(split(&answer).0, item(&node, "compat-term", ALL));
}

// AC-3
#[test]
fn a_failing_item_answers_its_own_error_and_the_others_are_unaffected() {
    let node = start();
    let bad = r#"{"query":{"no_such_query":{}}}"#;
    let body = format!(
        "{{\"index\":\"no-such-index\"}}\n{ALL}\n{{\"index\":\"compat-term\"}}\n{bad}\n{{\"index\":\"compat-term\"}}\n{TERM}\n"
    );
    let (status, answer) = msearch(&node, "/_msearch", &body);
    assert_eq!(status, 200, "{answer}");
    let (items, _) = split(&answer);
    assert!(items.starts_with(r#"{"error":{"#), "{items}");
    assert!(items.contains("index_not_found_exception"), "{items}");
    assert!(items.contains(r#""status":404}"#), "{items}");
    assert!(items.contains(r#""status":400}"#), "{items}");
    assert!(
        items.ends_with(&item(&node, "compat-term", TERM)),
        "{items}"
    );
}

// AC-4
#[test]
fn unsupported_headers_answer_a_refused_item() {
    let node = start();
    for header in [
        r#"{"index":"compat-term","preference":"_local"}"#,
        r#"{"index":["compat-term"]}"#,
        r#"{"index":"comp*"}"#,
        r#"{"index":"_all"}"#,
        r#"{"index":"compat-term,compat-bool"}"#,
        "{}",
    ] {
        let body = format!("{header}\n{ALL}\n");
        let (status, answer) = msearch(&node, "/_msearch", &body);
        assert_eq!(status, 200, "{header}: {answer}");
        assert!(
            answer.contains("illegal_argument_exception")
                && answer.contains("not supported by this node yet")
                && answer.contains(r#""status":400"#),
            "{header}: {answer}"
        );
    }
}

// AC-5
#[test]
fn an_empty_body_a_missing_body_line_and_a_missing_newline_are_refused() {
    let node = start();
    let (status, body) = msearch(&node, "/_msearch", "");
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains("action_request_validation_exception"),
        "{body}"
    );
    assert!(body.contains("no requests added"), "{body}");
    let (status, body) = msearch(&node, "/_msearch", "{\"index\":\"compat-term\"}\n");
    assert_eq!(status, 400, "{body}");
    let (status, body) = msearch(
        &node,
        "/_msearch",
        &format!("{{\"index\":\"compat-term\"}}\n{ALL}"),
    );
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("must be terminated by a newline"), "{body}");
    let first = format!("{{\"index\":\"compat-term\"}}\n{ALL}\n{{\"index\":\"compat-term\"}}\n");
    let (status, body) = msearch(&node, "/_msearch", &first);
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains(r#""status":200"#) && body.contains(r#""status":400"#),
        "{body}"
    );
}

// AC-6
#[cfg(feature = "coordinator")]
#[test]
fn an_item_over_two_shards_is_the_coordinators_response() {
    let node = start();
    put_variant(&node.store, "compat-term", "two", |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    put_shard_copy(&node.store, "compat-term", "compat-term-uuid", 1);
    let body = format!("{{\"index\":\"two\"}}\n{TERM}\n");
    let (status, answer) = msearch(&node, "/_msearch", &body);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(split(&answer).0, item(&node, "two", TERM));
}

// AC-7
#[test]
fn an_unknown_parameter_is_refused_and_a_warm_run_makes_no_request() {
    let node = start();
    let body = format!("{{\"index\":\"compat-term\"}}\n{ALL}\n");
    let (status, answer) = msearch(&node, "/_msearch?max_concurrent_searches=2", &body);
    assert_eq!(status, 400, "{answer}");
    assert_eq!(msearch(&node, "/_msearch", &body).0, 200);
    let warm = counts(&node.store);
    assert_eq!(msearch(&node, "/_msearch", &body).0, 200);
    assert_eq!(counts(&node.store), warm);
}

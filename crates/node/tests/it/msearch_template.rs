//! docs/specs/msearch-template: `_msearch/template` end to end.

use crate::support::{Node, counts, request, start};

const TERM: &str =
    r#"{"source":"{\"query\":{\"term\":{\"title\":\"{{t}}\"}}}","params":{"t":"brown"}}"#;
const TERM_SEARCH: &str = r#"{"query":{"term":{"title":"brown"}}}"#;
const ALL: &str = r#"{"source":"{\"query\":{\"match_all\":{}}}"}"#;
const ALL_SEARCH: &str = r#"{"query":{"match_all":{}}}"#;

fn post(node: &Node, path: &str, body: &str) -> (u16, String) {
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
    let (status, text) = post(node, &format!("/{index}/_search"), body);
    assert_eq!(status, 200, "{text}");
    let inner = text.strip_suffix('}').expect("an object");
    norm(&format!(r#"{inner},"status":200}}"#))
}

/// The items of an answer, joined, without its `took`.
fn items(answer: &str) -> String {
    let rest = answer
        .strip_prefix(r#"{"responses":["#)
        .unwrap_or_else(|| panic!("{answer}"));
    let (items, _) = rest.rsplit_once(r#"],"took":"#).expect("took");
    norm(items)
}

// AC-1
#[test]
fn each_item_is_its_searchs_response_with_a_status() {
    let node = start();
    let body =
        format!("{{\"index\":\"compat-term\"}}\n{TERM}\n{{\"index\":\"compat-term\"}}\n{ALL}\n");
    let (status, answer) = post(&node, "/_msearch/template", &body);
    assert_eq!(status, 200, "{answer}");
    let want = [
        item(&node, "compat-term", TERM_SEARCH),
        item(&node, "compat-term", ALL_SEARCH),
    ]
    .join(",");
    assert_eq!(items(&answer), want);
}

// AC-1
#[cfg(feature = "coordinator")]
#[test]
fn a_two_shard_index_searches_through_the_coordinator() {
    use crate::support::{put_shard_copy, put_variant};
    let node = start();
    put_variant(&node.store, "compat-term", "two", |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    put_shard_copy(&node.store, "compat-term", "compat-term-uuid", 1);
    let body = format!("{{\"index\":\"two\"}}\n{ALL}\n");
    let (status, answer) = post(&node, "/_msearch/template", &body);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(items(&answer), item(&node, "two", ALL_SEARCH));
}

// AC-2, AC-3
#[test]
fn a_bad_item_fails_alone_in_its_slot() {
    let node = start();
    let bad = [
        (r#"{"source":"{{#a}}"}"#, "script_exception"),
        (r#"{"params":{}}"#, "template is missing"),
        (r#"{"id":"stored"}"#, "not supported by this node yet"),
        (r#"{"source":"{}","profile":true}"#, "[profile]"),
    ];
    for (line, expect) in bad {
        let body = format!(
            "{{\"index\":\"compat-term\"}}\n{ALL}\n{{\"index\":\"compat-term\"}}\n{line}\n{{\"index\":\"compat-term\"}}\n{TERM}\n"
        );
        let (status, answer) = post(&node, "/_msearch/template", &body);
        assert_eq!(status, 200, "{answer}");
        let all = item(&node, "compat-term", ALL_SEARCH);
        let term = item(&node, "compat-term", TERM_SEARCH);
        let got = items(&answer);
        assert!(got.starts_with(&all), "{line}: {got}");
        assert!(got.ends_with(&term), "{line}: {got}");
        let middle = &got[all.len()..got.len() - term.len()];
        assert!(middle.contains(expect), "{line}: {middle}");
        assert!(middle.contains(r#""status":400"#), "{line}: {middle}");
    }
}

// AC-4
#[test]
fn the_paths_index_is_the_default() {
    let node = start();
    let body = format!("{{}}\n{TERM}\n{{\"index\":\"compat-term\"}}\n{ALL}\n");
    let (status, answer) = post(&node, "/compat-term/_msearch/template", &body);
    assert_eq!(status, 200, "{answer}");
    let want = [
        item(&node, "compat-term", TERM_SEARCH),
        item(&node, "compat-term", ALL_SEARCH),
    ]
    .join(",");
    assert_eq!(items(&answer), want);
    let (status, answer) = post(&node, "/_msearch/template", &format!("{{}}\n{TERM}\n"));
    assert_eq!(status, 200, "{answer}");
    assert!(
        answer.contains("an _msearch item with no index"),
        "{answer}"
    );
}

// AC-5
#[test]
fn an_empty_body_and_an_unterminated_one_are_the_errors_msearch_gives() {
    let node = start();
    let (status, answer) = post(&node, "/_msearch/template", "");
    assert_eq!(status, 400, "{answer}");
    assert!(answer.contains("no requests added"), "{answer}");
    let (status, answer) = post(&node, "/_msearch/template", "{\"index\":\"compat-term\"}\n");
    assert_eq!(status, 400, "{answer}");
    assert!(answer.contains("missing the body line"), "{answer}");
}

// AC-6
#[test]
fn a_warm_index_adds_no_store_request() {
    let node = start();
    let body = format!("{{\"index\":\"compat-term\"}}\n{ALL}\n");
    assert_eq!(post(&node, "/_msearch/template", &body).0, 200);
    let warm = counts(&node.store);
    assert_eq!(post(&node, "/_msearch/template", &body).0, 200);
    assert_eq!(counts(&node.store), warm);
}

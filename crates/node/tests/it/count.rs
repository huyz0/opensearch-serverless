//! docs/specs/count-api: `_count` end to end.

use crate::support::{Node, counts, request, start};
#[cfg(feature = "coordinator")]
use crate::support::{put_shard_copy, put_variant};

#[cfg(feature = "coordinator")]
fn two_shards() -> Node {
    let node = start();
    put_variant(&node.store, "compat-term", "two", |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    put_shard_copy(&node.store, "compat-term", "compat-term-uuid", 1);
    node
}

/// `hits.total.value` of `body`, a search response.
fn total_of(body: &str) -> u64 {
    let (_, rest) = body.split_once(r#""total":{"value":"#).expect("a total");
    rest.chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .expect("a number")
}

fn count_of(node: &Node, index: &str, body: Option<&str>) -> (u16, String) {
    let r = request(
        node.addr(),
        "POST",
        &format!("/{index}/_count"),
        body.map(str::as_bytes),
    );
    (r.status, r.text())
}

fn agrees(node: &Node, index: &str, query: &str) {
    let search = request(
        node.addr(),
        "POST",
        &format!("/{index}/_search"),
        Some(format!(r#"{{"query":{query},"size":0,"track_total_hits":true}}"#).as_bytes()),
    );
    assert_eq!(search.status, 200, "{}", search.text());
    let want = total_of(&search.text());
    let (status, got) = count_of(node, index, Some(&format!(r#"{{"query":{query}}}"#)));
    assert_eq!(status, 200, "{got}");
    let shards = search
        .text()
        .split_once(r#""_shards":"#)
        .expect("shards")
        .1
        .split_once('}')
        .expect("end")
        .0
        .to_owned();
    assert_eq!(
        got,
        format!(r#"{{"count":{want},"_shards":{shards}}}}}"#),
        "{query}"
    );
}

// AC-1
#[test]
fn a_count_equals_the_searchs_exact_total_over_one_shard() {
    let node = start();
    agrees(&node, "compat-term", r#"{"term":{"title":"brown"}}"#);
    agrees(&node, "compat-term", r#"{"match_all":{}}"#);
    agrees(&node, "compat-term", r#"{"term":{"title":"zzzz"}}"#);
}

// AC-2
#[cfg(feature = "coordinator")]
#[test]
fn a_count_equals_the_searchs_exact_total_over_two_shards() {
    let node = two_shards();
    agrees(&node, "two", r#"{"term":{"title":"brown"}}"#);
    agrees(&node, "two", r#"{"match_all":{}}"#);
}

// AC-3
#[test]
fn no_body_an_empty_body_and_match_all_count_everything_by_either_method() {
    let node = start();
    let (_, all) = count_of(&node, "compat-term", Some(r#"{"query":{"match_all":{}}}"#));
    assert!(all.starts_with(r#"{"count":"#), "{all}");
    for body in [None, Some("{}")] {
        assert_eq!(count_of(&node, "compat-term", body), (200, all.clone()));
    }
    let r = request(node.addr(), "GET", "/compat-term/_count", None);
    assert_eq!((r.status, r.text()), (200, all));
}

// AC-4
#[test]
fn an_unknown_index_is_not_found_as_in_a_search() {
    let node = start();
    let (status, body) = count_of(&node, "no-such-index", None);
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("index_not_found_exception"), "{body}");
}

// AC-5
#[test]
fn a_query_the_search_refuses_is_refused_the_same_way() {
    let node = start();
    let q = r#"{"query":{"no_such_query":{}}}"#;
    let search = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(q.as_bytes()),
    );
    let (status, body) = count_of(&node, "compat-term", Some(q));
    assert_eq!(status, search.status, "{body}");
    assert!(status >= 400, "{body}");
    let kind = |s: &str| {
        s.split_once(r#""type":""#)
            .map(|(_, r)| r.split('"').next().unwrap_or_default().to_owned())
    };
    assert_eq!(kind(&body), kind(&search.text()));
}

// AC-6
#[test]
fn a_body_key_other_than_query_and_an_unknown_parameter_are_refused() {
    let node = start();
    for key in [
        r#""size":1"#,
        r#""from":1"#,
        r#""sort":["_doc"]"#,
        r#""aggs":{"a":{"max":{"field":"n"}}}"#,
    ] {
        let (status, body) = count_of(&node, "compat-term", Some(&format!("{{{key}}}")));
        assert_eq!(status, 400, "{key}: {body}");
        assert!(body.contains("illegal_argument_exception"), "{key}: {body}");
        assert!(body.contains("_count"), "{key}: {body}");
    }
    let r = request(node.addr(), "POST", "/compat-term/_count?q=a", None);
    assert_eq!(r.status, 400, "{}", r.text());
}

// AC-7
#[test]
fn a_count_of_an_open_index_makes_no_request_to_the_store() {
    let node = start();
    assert_eq!(count_of(&node, "compat-term", None).0, 200);
    let warm = counts(&node.store);
    assert_eq!(count_of(&node, "compat-term", None).0, 200);
    assert_eq!(counts(&node.store), warm);
}

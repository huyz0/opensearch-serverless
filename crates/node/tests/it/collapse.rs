//! docs/specs/search-collapse: `collapse`, end to end over the compat blobs.
//! The index holds ids 1..5: tags animal, animal, manual, food, animal and
//! years 2001, 2002, 2010, 2002, 1999.

use crate::support::{counts, request, start};

fn run(node: &crate::support::Node, body: &str) -> (u16, String) {
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

fn ids(body: &str) -> Vec<String> {
    body.split(r#""_id":""#)
        .skip(1)
        .map(|s| s[..s.find('"').expect("id")].to_owned())
        .collect()
}

// AC-1
#[test]
fn a_keyword_keeps_the_best_hit_of_each_value() {
    let node = start();
    let (status, body) = run(
        &node,
        r#"{"query":{"match_all":{}},"collapse":{"field":"tag"}}"#,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(ids(&body), ["1", "3", "4"], "{body}");
    assert!(body.contains(r#""total":{"value":5"#), "{body}");
    assert!(body.contains(r#""fields":{"tag":["animal"]}"#), "{body}");
    assert!(body.contains(r#""fields":{"tag":["manual"]}"#), "{body}");
}

// AC-2
#[test]
fn from_and_size_page_the_groups() {
    let node = start();
    let (_, body) = run(
        &node,
        r#"{"from":1,"size":1,"query":{"match_all":{}},"collapse":{"field":"tag"}}"#,
    );
    assert_eq!(ids(&body), ["3"], "{body}");
    let (_, body) = run(
        &node,
        r#"{"from":2,"size":5,"query":{"match_all":{}},"collapse":{"field":"tag"}}"#,
    );
    assert_eq!(ids(&body), ["4"], "{body}");
}

// AC-3
#[test]
fn a_sort_orders_the_groups_by_their_best_hit() {
    let node = start();
    let (status, body) = run(
        &node,
        r#"{"query":{"match_all":{}},"sort":[{"year":"asc"}],"collapse":{"field":"tag"}}"#,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(ids(&body), ["5", "4", "3"], "{body}");
    assert!(body.contains(r#""sort":[1999]"#), "{body}");
    assert!(body.contains(r#""fields":{"tag":["food"]}"#), "{body}");
}

// AC-4
#[test]
fn a_number_collapses_the_same_way() {
    let node = start();
    let (status, body) = run(
        &node,
        r#"{"query":{"match_all":{}},"collapse":{"field":"year"}}"#,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(ids(&body), ["1", "2", "3", "5"], "{body}");
    assert!(body.contains(r#""fields":{"year":[2002]}"#), "{body}");
}

// AC-5
#[test]
fn what_stock_refuses_is_refused() {
    let node = start();
    for (tail, reason) in [
        (
            r#""collapse":{"field":"nope"}"#,
            "no mapping found for `nope` in order to collapse on",
        ),
        (
            r#""collapse":{"field":"title"}"#,
            "unknown type for collapse field `title`, only keywords and numbers are accepted",
        ),
        (
            r#""collapse":{"field":"published"}"#,
            "unknown type for collapse field `published`",
        ),
        (
            r#""collapse":{"field":"tag"},"rescore":{"query":{"rescore_query":{"match_all":{}}}}"#,
            "cannot use `collapse` in conjunction with `rescore`",
        ),
        (
            r#""collapse":{"field":"tag","inner_hits":{"name":"n"}}"#,
            "[collapse] [inner_hits] is not supported",
        ),
        (
            r#""collapse":{"field":"tag"},"sort":["tag"],"search_after":["a"]"#,
            "[collapse] with [search_after] is not supported",
        ),
    ] {
        let (status, body) = run(
            &node,
            &format!(r#"{{"query":{{"match_all":{{}}}},{tail}}}"#),
        );
        assert_eq!(status, 400, "{tail}: {body}");
        assert!(body.contains(reason), "{tail}: {body}");
    }
}

// AC-7
#[test]
fn a_collapse_adds_no_warm_store_request() {
    let node = start();
    let cost = |body: &str| {
        run(&node, body);
        let before = counts(&node.store);
        run(&node, body);
        counts(&node.store)
            .iter()
            .zip(before.iter())
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        cost(r#"{"query":{"match_all":{}},"collapse":{"field":"tag"}}"#),
        cost(r#"{"query":{"match_all":{}},"docvalue_fields":["tag"]}"#),
    );
}

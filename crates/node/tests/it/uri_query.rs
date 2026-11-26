//! docs/specs/uri-query: the `q` URL parameter on `_search` and `_count`.

use crate::support::{Node, request, start};
#[cfg(feature = "coordinator")]
use crate::support::{put_shard_copy, put_variant};

/// `hits.total.value` of a search response.
fn total_of(body: &str) -> u64 {
    let (_, rest) = body.split_once(r#""total":{"value":"#).expect("a total");
    rest.chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .expect("a number")
}

fn search(node: &Node, path: &str, body: Option<&str>) -> (u16, String) {
    let r = request(node.addr(), "POST", path, body.map(str::as_bytes));
    (r.status, r.text())
}

/// The total of `/compat-term/_search?{url}` with `body`.
fn searched(node: &Node, url: &str, body: Option<&str>) -> u64 {
    let (status, text) = search(node, &format!("/compat-term/_search?{url}"), body);
    assert_eq!(status, 200, "{url}: {text}");
    total_of(&text)
}

fn count_of(text: &str) -> u64 {
    let (_, rest) = text.split_once(r#""count":"#).expect("a count");
    rest.chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .expect("a number")
}

const BY_BODY: &str =
    r#"{"track_total_hits":true,"query":{"query_string":{"query":"title:brown"}}}"#;

// AC-1
#[test]
fn q_searches_as_the_same_query_string_in_the_body() {
    let node = start();
    let all = searched(&node, "", Some(r#"{"track_total_hits":true}"#));
    let want = searched(&node, "", Some(BY_BODY));
    assert!(want > 0 && want < all, "{want} of {all} makes a real check");
    assert_eq!(searched(&node, "q=title:brown", None), want);
}

// AC-2
#[test]
fn q_replaces_a_query_in_the_body_of_a_search() {
    let node = start();
    let want = searched(&node, "", Some(BY_BODY));
    let body = r#"{"track_total_hits":true,"query":{"match_all":{}}}"#;
    assert_eq!(searched(&node, "q=title:brown", Some(body)), want);
}

// AC-3
#[test]
fn the_companions_shape_the_query() {
    let node = start();
    let any = searched(&node, "q=brown%20quick&df=title", None);
    let both = searched(&node, "q=brown%20quick&df=title&default_operator=and", None);
    assert!(both <= any, "{both} and {any}");
    let by_body = |operator: &str| {
        searched(
            &node,
            "",
            Some(&format!(
                r#"{{"query":{{"query_string":{{"query":"brown quick","default_field":"title","default_operator":"{operator}"}}}}}}"#
            )),
        )
    };
    assert_eq!(any, by_body("or"));
    assert_eq!(both, by_body("and"));
    assert_eq!(
        searched(&node, "q=title:brown&lenient=true", None),
        searched(&node, "q=title:brown", None)
    );
}

// AC-4
#[test]
fn a_count_with_q_counts_that_query() {
    let node = start();
    let want = searched(&node, "", Some(BY_BODY));
    let (status, text) = search(&node, "/compat-term/_count?q=title:brown", None);
    assert_eq!(status, 200, "{text}");
    assert_eq!(count_of(&text), want);
    let (status, text) = search(
        &node,
        "/compat-term/_count?q=title:brown",
        Some(r#"{"query":{"match_all":{}}}"#),
    );
    assert_eq!(status, 200, "{text}");
    let (_, all) = search(&node, "/compat-term/_count", None);
    assert_eq!(count_of(&text), count_of(&all), "a body query wins over q");
}

// AC-5
#[cfg(feature = "coordinator")]
#[test]
fn q_counts_and_searches_the_same_over_two_shards() {
    let node = start();
    put_variant(&node.store, "compat-term", "two", |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    put_shard_copy(&node.store, "compat-term", "compat-term-uuid", 1);
    let want = searched(&node, "", Some(BY_BODY));
    assert_eq!(searched(&node, "q=title:brown", None), want);
    let (status, text) = search(&node, "/compat-term/_count?q=title:brown", None);
    assert_eq!(status, 200, "{text}");
    assert_eq!(count_of(&text), want);
}

// AC-6
#[test]
fn a_q_that_cannot_be_parsed_or_a_bad_flag_is_a_400() {
    let node = start();
    for url in [
        "/compat-term/_search?q=title:(brown",
        "/compat-term/_search?q=a&lenient=maybe",
        "/compat-term/_search?q=a&default_operator=xor",
        "/compat-term/_count?q=a&analyze_wildcard=1",
    ] {
        let (status, text) = search(&node, url, None);
        assert_eq!(status, 400, "{url}: {text}");
    }
}

//! docs/specs/validate-query-api: `_validate/query` end to end.

use crate::support::{Node, counts, request, start};
#[cfg(feature = "coordinator")]
use crate::support::{put_shard_copy, put_variant};

const VALID: &str = r#"{"_shards":{"total":1,"successful":1,"failed":0},"valid":true}"#;
const INVALID: &str = r#"{"_shards":{"total":1,"successful":1,"failed":0},"valid":false}"#;

fn validate(node: &Node, method: &str, index: &str, body: Option<&str>) -> (u16, String) {
    let r = request(
        node.addr(),
        method,
        &format!("/{index}/_validate/query"),
        body.map(str::as_bytes),
    );
    (r.status, r.text())
}

// AC-1
#[test]
fn a_query_the_search_accepts_is_valid() {
    let node = start();
    for q in [
        r#"{"term":{"title":"brown"}}"#,
        r#"{"match":{"title":"quick fox"}}"#,
        r#"{"bool":{"must":[{"match_all":{}}]}}"#,
    ] {
        let body = format!(r#"{{"query":{q}}}"#);
        assert_eq!(
            validate(&node, "POST", "compat-term", Some(&body)),
            (200, VALID.to_owned()),
            "{q}"
        );
    }
}

// AC-1
#[cfg(feature = "coordinator")]
#[test]
fn a_two_shard_index_validates_against_one_shard() {
    let node = start();
    put_variant(&node.store, "compat-term", "two", |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    put_shard_copy(&node.store, "compat-term", "compat-term-uuid", 1);
    let body = r#"{"query":{"term":{"title":"brown"}}}"#;
    assert_eq!(
        validate(&node, "POST", "two", Some(body)),
        (200, VALID.to_owned())
    );
}

// AC-2
#[test]
fn no_body_an_empty_body_and_match_all_are_valid_by_either_method() {
    let node = start();
    for method in ["GET", "POST"] {
        for body in [None, Some("{}"), Some(r#"{"query":{"match_all":{}}}"#)] {
            assert_eq!(
                validate(&node, method, "compat-term", body),
                (200, VALID.to_owned()),
                "{method} {body:?}"
            );
        }
    }
}

// AC-3
#[test]
fn a_body_that_does_not_parse_is_not_valid() {
    let node = start();
    for body in [
        r#"{"query":{"no_such_query":{}}}"#,
        r#"{"nope":1}"#,
        r#"{"size":1}"#,
        r#"{"query":{"match_all":{}},"from":1}"#,
        "not json",
        r#"{"query":"#,
    ] {
        assert_eq!(
            validate(&node, "POST", "compat-term", Some(body)),
            (200, INVALID.to_owned()),
            "{body}"
        );
    }
}

// AC-4
#[test]
fn a_query_that_fails_to_build_is_not_valid() {
    let node = start();
    let body = r#"{"query":{"prefix":{"year":"1"}}}"#;
    assert_eq!(
        validate(&node, "POST", "compat-term", Some(body)),
        (200, INVALID.to_owned())
    );
}

// AC-4
#[test]
fn a_query_this_node_does_not_support_is_refused_not_invalid() {
    let node = start();
    let body = r#"{"query":{"geo_shape":{"p":{"shape":{"type":"point","coordinates":[0,0]}}}}}"#;
    let (status, text) = validate(&node, "POST", "compat-term", Some(body));
    assert_eq!(status, 400, "{text}");
    assert!(text.contains("illegal_argument_exception"), "{text}");
    assert!(text.contains("not supported by this node yet"), "{text}");
}

// AC-5
#[test]
fn an_unknown_index_is_not_found_as_in_a_search() {
    let node = start();
    let (status, body) = validate(&node, "POST", "no-such-index", None);
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("index_not_found_exception"), "{body}");
}

// AC-6
#[test]
fn no_index_and_any_parameter_are_refused() {
    let node = start();
    let r = request(node.addr(), "POST", "/_validate/query", None);
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("not supported by this node yet"));
    for param in ["explain=true", "rewrite=true", "all_shards=true"] {
        let r = request(
            node.addr(),
            "POST",
            &format!("/compat-term/_validate/query?{param}"),
            None,
        );
        assert_eq!(r.status, 400, "{param}: {}", r.text());
    }
}

// AC-7
#[test]
fn validating_an_open_index_makes_no_request_to_the_store() {
    let node = start();
    assert_eq!(validate(&node, "POST", "compat-term", None).0, 200);
    let warm = counts(&node.store);
    assert_eq!(validate(&node, "POST", "compat-term", None).0, 200);
    assert_eq!(counts(&node.store), warm);
}

fn validate_with(node: &Node, query: &str, body: Option<&str>) -> (u16, String) {
    let r = request(
        node.addr(),
        "POST",
        &format!("/compat-term/_validate/query{query}"),
        body.map(str::as_bytes),
    );
    (r.status, r.text())
}

// AC-8
#[test]
fn the_q_parameters_are_validated_as_a_query_string() {
    let node = start();
    for good in [
        "?q=title:brown",
        "?q=brown&df=title",
        "?q=quick%20fox&df=title&default_operator=AND",
        "?q=brown&df=title&analyzer=standard&lenient=true",
    ] {
        assert_eq!(
            validate_with(&node, good, None),
            (200, VALID.to_owned()),
            "{good}"
        );
    }
    // A query string that does not parse is invalid, not a refusal.
    assert_eq!(
        validate_with(&node, "?q=title:(brown", None),
        (200, INVALID.to_owned())
    );
    // A body's query wins over `q`.
    assert_eq!(
        validate_with(
            &node,
            "?q=title:(brown",
            Some(r#"{"query":{"match_all":{}}}"#)
        ),
        (200, VALID.to_owned())
    );
    // What `query_string` does not serve is refused as in a search.
    let (status, body) = validate_with(&node, "?q=a&analyze_wildcard=true", None);
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("not supported by this node yet"), "{body}");
    for bad in ["?q=a&default_operator=XOR", "?q=a&lenient=maybe"] {
        let (status, body) = validate_with(&node, bad, None);
        assert_eq!(status, 400, "{bad}: {body}");
        assert!(body.contains("illegal_argument_exception"), "{bad}: {body}");
    }
}

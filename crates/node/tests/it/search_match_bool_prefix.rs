//! docs/specs/query-match-bool-prefix: the `match_bool_prefix` query through HTTP.

use crate::support::{request, start};

fn total(query: &str) -> String {
    let node = start();
    let body = format!(r#"{{"query":{query},"size":0}}"#);
    let r = request(
        node.addr(),
        "POST",
        "/compat-paging-sort/_search",
        Some(body.as_bytes()),
    );
    assert_eq!(r.status, 200, "{query}: {}", r.text());
    r.text()
}

#[test]
fn the_last_term_matches_by_prefix() {
    let or = total(r#"{"match_bool_prefix":{"title":"quick fo"}}"#);
    assert!(
        or.contains(r#""total":{"value":3,"relation":"eq"}"#),
        "{or}"
    );
    let and = total(r#"{"match_bool_prefix":{"title":{"query":"quick fo","operator":"and"}}}"#);
    assert!(
        and.contains(r#""total":{"value":1,"relation":"eq"}"#),
        "{and}"
    );
    let tag = total(r#"{"match_bool_prefix":{"tag":"ani"}}"#);
    assert!(
        tag.contains(r#""total":{"value":3,"relation":"eq"}"#),
        "{tag}"
    );
}

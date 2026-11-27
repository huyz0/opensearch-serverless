//! docs/specs/query-match-phrase-prefix: the `match_phrase_prefix` query through HTTP.

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
fn the_last_term_matches_by_prefix_in_a_phrase() {
    let phrase = total(r#"{"match_phrase_prefix":{"title":"brown fo"}}"#);
    assert!(
        phrase.contains(r#""total":{"value":1,"relation":"eq"}"#),
        "{phrase}"
    );
    let single = total(r#"{"match_phrase_prefix":{"title":"qu"}}"#);
    assert!(
        single.contains(r#""total":{"value":2,"relation":"eq"}"#),
        "{single}"
    );
    let none = total(r#"{"match_phrase_prefix":{"title":"fox quick"}}"#);
    assert!(
        none.contains(r#""total":{"value":0,"relation":"eq"}"#),
        "{none}"
    );
}

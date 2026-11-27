//! docs/specs/query-case-insensitive: `case_insensitive` on term, prefix and wildcard through HTTP.

use crate::support::{request, start};

fn total(body: &str) -> (u16, String) {
    let node = start();
    let r = request(
        node.addr(),
        "POST",
        "/compat-paging-sort/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

fn hits(query: &str) -> String {
    let (status, text) = total(&format!(r#"{{"query":{query},"size":0}}"#));
    assert_eq!(status, 200, "{query}: {text}");
    text
}

#[test]
fn a_term_prefix_or_wildcard_ignores_case_when_asked() {
    for (query, one) in [
        (
            r#"{"term":{"tag":{"value":"MANUAL","case_insensitive":true}}}"#,
            true,
        ),
        (r#"{"term":{"tag":{"value":"MANUAL"}}}"#, false),
        (
            r#"{"term":{"tag":{"value":"MANUAL","case_insensitive":false}}}"#,
            false,
        ),
        (
            r#"{"prefix":{"tag":{"value":"MAN","case_insensitive":true}}}"#,
            true,
        ),
        (r#"{"prefix":{"tag":{"value":"MAN"}}}"#, false),
        (
            r#"{"wildcard":{"tag":{"value":"M*A?","case_insensitive":true}}}"#,
            true,
        ),
        (r#"{"wildcard":{"tag":{"value":"M*A?"}}}"#, false),
    ] {
        let text = hits(query);
        let expect = if one {
            r#""total":{"value":1,"relation":"eq"}"#
        } else {
            r#""total":{"value":0,"relation":"eq"}"#
        };
        assert!(text.contains(expect), "{query}: {text}");
    }
}

#[test]
fn what_stays_refused_is_refused() {
    for query in [
        r#"{"regexp":{"tag":{"value":"man.*","case_insensitive":true}}}"#,
        r#"{"term":{"year":{"value":"1","case_insensitive":true}}}"#,
    ] {
        let (status, text) = total(&format!(r#"{{"query":{query}}}"#));
        assert_eq!(status, 400, "{query}: {text}");
    }
}

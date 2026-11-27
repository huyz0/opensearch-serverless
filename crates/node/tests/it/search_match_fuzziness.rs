//! docs/specs/query-match-fuzziness: `fuzziness` on a `match` query through HTTP.

use crate::support::{request, start};

fn post(body: &str) -> (u16, String) {
    let node = start();
    let r = request(
        node.addr(),
        "POST",
        "/compat-paging-sort/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

#[test]
fn a_misspelt_term_matches_with_fuzziness() {
    let (status, text) = post(r#"{"query":{"match":{"title":{"query":"quikc","fuzziness":1}}}}"#);
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(r#""total":{"value":2,"relation":"eq"}"#), "{text}");
    let (status, text) = post(r#"{"query":{"match":{"title":"quikc"}}}"#);
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(r#""total":{"value":0,"relation":"eq"}"#), "{text}");
    let (status, text) = post(r#"{"query":{"match":{"title":{"query":"quikc","fuzziness":0}}}}"#);
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(r#""total":{"value":0,"relation":"eq"}"#), "{text}");
}

#[test]
fn what_stays_refused_is_refused() {
    let (status, text) =
        post(r#"{"query":{"match":{"title":{"query":"quikc","fuzziness":1,"fuzzy_rewrite":"x"}}}}"#);
    assert_eq!(status, 400, "{text}");
    assert!(text.contains("is not supported by this node yet"), "{text}");
}

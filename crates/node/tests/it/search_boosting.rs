//! docs/specs/query-boosting: the `boosting` query through HTTP.

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
fn a_negative_match_scales_the_score_and_the_rest_keep_it() {
    let (status, text) = post(
        r#"{"query":{"boosting":{"positive":{"match_all":{}},"negative":{"term":{"tag":"manual"}},"negative_boost":0.5}}}"#,
    );
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(r#""_id":"3","_score":0.5"#), "{text}");
    assert!(text.contains(r#""_score":1.0"#), "{text}");
}

#[test]
fn a_missing_part_is_refused_as_opensearch_refuses_it() {
    let (status, text) = post(r#"{"query":{"boosting":{"positive":{"match_all":{}}}}}"#);
    assert_eq!(status, 400, "{text}");
    assert!(
        text.contains("requires 'negative' query to be set"),
        "{text}"
    );
}

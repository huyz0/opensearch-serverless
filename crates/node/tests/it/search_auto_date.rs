//! docs/specs/auto-date-histogram: an `auto_date_histogram` over a REST search.

use crate::support::{request, start};

const PATH: &str = "/compat-paging-sort/_search";

fn post(query: &str, body: &str) -> (u16, String) {
    let node = start();
    let r = request(
        node.addr(),
        "POST",
        &format!("{PATH}?{query}"),
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

const AUTO: &str =
    r#"{"size":0,"aggs":{"when":{"auto_date_histogram":{"field":"year","buckets":5}}}}"#;

/// docs/specs/auto-date-histogram AC-5: the aggregation answers with its
/// interval, typed or not, and a bad body is a 400.
#[test]
fn an_auto_date_histogram_picks_its_interval() {
    let (status, plain) = post("", AUTO);
    assert_eq!(status, 200, "{plain}");
    assert!(plain.contains(r#""when":{"buckets":[{"#), "{plain}");
    assert!(plain.contains(r#""interval":"1s""#), "{plain}");
    let (status, typed) = post("typed_keys=true", AUTO);
    assert_eq!(status, 200, "{typed}");
    assert!(typed.contains(r#""auto_date_histogram#when":{"#), "{typed}");
    let (status, text) = post("", &AUTO.replace(r#""buckets":5"#, r#""buckets":0"#));
    assert_eq!(status, 400, "{text}");
    assert!(
        text.contains("buckets must be greater than 0 for [when]"),
        "{text}"
    );
}

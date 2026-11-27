//! docs/specs/pipeline-aggs-sibling: sibling pipeline aggregations over a REST search.

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

const SIBLING: &str = r#"{"size":0,"aggs":{"by":{"terms":{"field":"year"}},"most":{"max_bucket":{"buckets_path":"by>_count"}},"mean":{"avg_bucket":{"buckets_path":"by>_count"}}}}"#;

/// docs/specs/pipeline-aggs-sibling AC-10: a sibling pipeline aggregation
/// over a REST search, typed or not, and its validation failure.
#[test]
fn a_sibling_pipeline_aggregation_is_written_beside_the_aggregation_it_reads() {
    let (status, plain) = post("", SIBLING);
    assert_eq!(status, 200, "{plain}");
    assert!(plain.contains(r#""most":{"value":"#), "{plain}");
    assert!(plain.contains(r#""keys":["#), "{plain}");
    assert!(plain.contains(r#""mean":{"value":"#), "{plain}");
    let (status, typed) = post("typed_keys=true", SIBLING);
    assert_eq!(status, 200, "{typed}");
    assert!(typed.contains(r#""bucket_metric_value#most":{"#), "{typed}");
    assert!(typed.contains(r#""simple_value#mean":{"#), "{typed}");
    let bad = SIBLING.replace("by>_count", "nope>_count");
    let (status, text) = post("", &bad);
    assert_eq!(status, 400, "{text}");
    assert!(
        text.contains("Validation Failed: 1: No aggregation found for path [nope>_count];"),
        "{text}"
    );
}

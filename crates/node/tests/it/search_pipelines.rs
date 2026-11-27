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

const PARENT: &str = r#"{"size":0,"aggs":{"h":{"histogram":{"field":"year","interval":10},"aggs":{"s":{"sum":{"field":"year"}},"d":{"derivative":{"buckets_path":"s"}},"c":{"cumulative_sum":{"buckets_path":"_count"}}}}}}"#;

/// docs/specs/pipeline-aggs-parent AC-10: parent pipeline aggregations inside
/// a histogram over a REST search, typed or not, trimmed by a `bucket_sort`,
/// and the validation failure of one with nothing to read.
#[test]
fn a_parent_pipeline_aggregation_works_on_the_buckets_that_hold_it() {
    let (status, plain) = post("", PARENT);
    assert_eq!(status, 200, "{plain}");
    assert!(plain.contains(r#""d":{"value":"#), "{plain}");
    assert!(plain.contains(r#""c":{"value":"#), "{plain}");
    let (status, typed) = post("typed_keys=true", PARENT);
    assert_eq!(status, 200, "{typed}");
    assert!(typed.contains(r#""simple_value#d":{"value":"#), "{typed}");
    assert!(typed.contains(r#""simple_value#c":{"value":"#), "{typed}");
    let sorted = PARENT.replace(
        r#""c":{"cumulative_sum":{"buckets_path":"_count"}}"#,
        r#""top":{"bucket_sort":{"sort":[{"_count":"desc"}],"size":1}}"#,
    );
    let (status, text) = post("", &sorted);
    assert_eq!(status, 200, "{text}");
    assert_eq!(text.matches(r#""doc_count":"#).count(), 1, "{text}");
    let bad = PARENT.replace(r#""buckets_path":"s""#, r#""buckets_path":"nope""#);
    let (status, text) = post("", &bad);
    assert_eq!(status, 400, "{text}");
    assert!(
        text.contains("Validation Failed: 1: No aggregation found for path [nope];"),
        "{text}"
    );
}

const SERIAL: &str = r#"{"size":0,"aggs":{"h":{"histogram":{"field":"year","interval":10},"aggs":{"s":{"sum":{"field":"year"}},"sd":{"serial_diff":{"buckets_path":"s","lag":1}}}}}}"#;

/// docs/specs/pipeline-aggs-serial-diff AC-6: a `serial_diff` inside a
/// histogram over a REST search, typed or not, and its refusals.
#[test]
fn a_serial_diff_works_on_the_buckets_that_hold_it() {
    let (status, plain) = post("", SERIAL);
    assert_eq!(status, 200, "{plain}");
    assert!(plain.contains(r#""sd":{"value":"#), "{plain}");
    let (status, typed) = post("typed_keys=true", SERIAL);
    assert_eq!(status, 200, "{typed}");
    assert!(typed.contains(r#""simple_value#sd":{"value":"#), "{typed}");
    let (status, text) = post("", &SERIAL.replace(r#""lag":1"#, r#""lag":0"#));
    assert_eq!(status, 400, "{text}");
    assert!(
        text.contains("Lag must be a positive, non-zero integer."),
        "{text}"
    );
    let (status, text) = post(
        "",
        &SERIAL.replace(r#""lag":1"#, r#""lag":1,"format":"0.0""#),
    );
    assert_eq!(status, 400, "{text}");
    assert!(text.contains("not supported by this node yet"), "{text}");
}

const ADJACENCY: &str = r#"{"size":0,"aggs":{"adj":{"adjacency_matrix":{"filters":{"all":{"match_all":{}},"none":{"term":{"year":-1}}}}}}}"#;

/// docs/specs/adjacency-matrix-agg AC-6: an `adjacency_matrix` over a REST
/// search, typed or not, and its refusals.
#[test]
fn an_adjacency_matrix_counts_each_filter_and_each_pair() {
    let (status, plain) = post("", ADJACENCY);
    assert_eq!(status, 200, "{plain}");
    assert!(
        plain.contains(r#""adj":{"buckets":[{"key":"all","doc_count":"#),
        "{plain}"
    );
    assert!(!plain.contains(r#""key":"none""#), "{plain}");
    let (status, typed) = post("typed_keys=true", ADJACENCY);
    assert_eq!(status, 200, "{typed}");
    assert!(
        typed.contains(r#""adjacency_matrix#adj":{"buckets":["#),
        "{typed}"
    );
    let (status, text) = post(
        "",
        &ADJACENCY.replace(r#""filters":{"all""#, r#""filters":[],"x":{"all""#),
    );
    assert_eq!(status, 400, "{text}");
}

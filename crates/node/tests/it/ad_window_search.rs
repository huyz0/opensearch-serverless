//! docs/specs/anomaly-detection AC-7: a detector's windows come from one
//! search of the node's own reader, and read back as the windows they are.

use oss_node::ad_route::{WindowSearch, windows};
use serde_json::{Value, json};

use crate::support::start;

/// 2001-03-03T00:00:00Z.
const FROM: i64 = 983_577_600_000;
const DAY: i64 = 86_400_000;

fn detector(filter: Value) -> Value {
    json!({
        "time_field": "published",
        "filter_query": filter,
        "detection_interval": {"period": {"interval": 1440, "unit": "Minutes"}},
        "feature_attributes": [
            {"feature_id": "latest", "feature_name": "latest", "feature_enabled": true,
             "aggregation_query": {"latest": {"max": {"field": "year"}}}},
            {"feature_id": "docs", "feature_name": "docs", "feature_enabled": true,
             "aggregation_query": {"docs": {"value_count": {"field": "year"}}}}
        ]
    })
}

fn windows_of(filter: Value, span: &windows::Span) -> Vec<Option<Vec<f32>>> {
    let node = start();
    let d = detector(filter);
    let body = windows::request(&d, span).expect("a request");
    let answer = node
        .service
        .search(&["compat-paging-sort".to_owned()], &body)
        .expect("the reader answers the detector's search");
    windows::parse(&answer, &d, span).expect("the answer holds the windows")
}

/// The reader runs the histogram the detector asks for, offset to the start
/// of the span, and the window with a document holds its features.
#[test]
fn the_reader_answers_a_detectors_windows() {
    let span = windows::Span {
        from_ms: FROM,
        interval_ms: DAY,
        count: 4,
    };
    let got = windows_of(json!({"match_all": {}}), &span);
    assert_eq!(got, vec![None, Some(vec![2001.0, 1.0]), None, None]);
}

/// A span that does not start on a day boundary still starts where it says,
/// and the detector's filter narrows what each window holds.
#[test]
fn an_offset_span_and_a_filter_are_honoured() {
    let span = windows::Span {
        from_ms: FROM + 3_600_000 * 5,
        interval_ms: DAY,
        count: 4,
    };
    let got = windows_of(json!({"match_all": {}}), &span);
    // the document at 2001-03-04T00:00Z falls in the first window, which starts at 05:00 the day before
    assert_eq!(
        got,
        vec![Some(vec![2001.0, 1.0]), None, None, None],
        "{got:?}"
    );
    let none = windows_of(json!({"term": {"tag": "no-such-tag"}}), &span);
    assert_eq!(none, vec![None; 4]);
}

/// A category field is a `terms` level of the same one search, and the reader
/// answers each entity's windows.
#[test]
fn the_reader_answers_each_entitys_windows() {
    let node = start();
    let mut d = detector(json!({"match_all": {}}));
    d["category_field"] = json!(["tag"]);
    // 1999-12-31T00:00:00Z: the day of the one document with the tag `animal` that year
    let span = windows::Span {
        from_ms: 946_598_400_000,
        interval_ms: DAY,
        count: 3,
    };
    let body = windows::request(&d, &span).expect("a request");
    let answer = node
        .service
        .search(&["compat-paging-sort".to_owned()], &body)
        .expect("the reader answers");
    let got = windows::parse_entities(&answer, &d, &span).expect("entities");
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].key, [("tag".to_owned(), "animal".to_owned())]);
    assert_eq!(got[0].windows, vec![Some(vec![1999.0, 1.0]), None, None]);
}

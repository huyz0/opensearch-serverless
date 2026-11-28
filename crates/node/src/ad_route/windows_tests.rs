use serde_json::{Value, json};

use super::{Span, features, interval_ms, parse, request};

fn detector() -> Value {
    json!({
        "time_field": "ts",
        "filter_query": {"term": {"host": "a"}},
        "detection_interval": {"period": {"interval": 10, "unit": "Minutes"}},
        "feature_attributes": [
            {"feature_id": "f1", "feature_name": "cpu", "feature_enabled": true,
             "aggregation_query": {"cpu": {"max": {"field": "cpu"}}}},
            {"feature_id": "f2", "feature_name": "off", "feature_enabled": false,
             "aggregation_query": {"off": {"min": {"field": "x"}}}},
            {"feature_id": "f3", "feature_name": "mem", "feature_enabled": true,
             "aggregation_query": {"mem": {"avg": {"field": "mem"}}}}
        ]
    })
}

const SPAN: Span = Span {
    from_ms: 1_200_000,
    interval_ms: 600_000,
    count: 4,
};

#[test]
fn only_enabled_features_are_read_in_order_and_the_interval_is_in_milliseconds() {
    let f = features(&detector()).unwrap();
    let ids: Vec<&str> = f.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["f1", "f3"]);
    assert_eq!(f[0].agg, json!({"max": {"field": "cpu"}}));
    assert_eq!(interval_ms(&detector()), Ok(600_000));
    assert!(interval_ms(&json!({})).is_err());
}

#[test]
fn the_request_is_one_histogram_of_the_span_with_each_feature_under_it() {
    let body = request(&detector(), &SPAN).unwrap();
    assert_eq!(body["size"], 0);
    let h = &body["aggs"]["windows"];
    assert_eq!(h["date_histogram"]["field"], "ts");
    assert_eq!(h["date_histogram"]["fixed_interval"], "600000ms");
    // 1,200,000 is two intervals past a multiple of 600,000: no offset.
    assert_eq!(h["date_histogram"]["offset"], "0ms");
    assert_eq!(
        h["date_histogram"]["extended_bounds"],
        json!({"min": 1_200_000, "max": 3_599_999})
    );
    assert_eq!(h["aggs"]["f1"], json!({"max": {"field": "cpu"}}));
    assert_eq!(h["aggs"]["f3"], json!({"avg": {"field": "mem"}}));
    assert!(h["aggs"].get("f2").is_none());
    let filter = &body["query"]["bool"]["filter"];
    assert_eq!(
        filter[0],
        json!({"range": {"ts": {"gte": 1_200_000, "lt": 3_600_000}}})
    );
    assert_eq!(filter[1], json!({"term": {"host": "a"}}));
    // a span that does not start on the interval is offset to start where it starts
    let odd = Span {
        from_ms: 1_234_567,
        ..SPAN
    };
    let body = request(&detector(), &odd).unwrap();
    assert_eq!(
        body["aggs"]["windows"]["date_histogram"]["offset"],
        "34567ms"
    );
}

#[test]
fn a_bucket_lands_in_its_window_and_a_window_without_data_is_missing() {
    let answer = json!({"aggregations": {"windows": {"buckets": [
        {"key": 1_200_000, "doc_count": 3, "f1": {"value": 5.0}, "f3": {"value": 1.5}},
        {"key": 1_800_000, "doc_count": 0, "f1": {"value": 0.0}, "f3": {"value": 0.0}},
        {"key": 2_400_000, "doc_count": 2, "f1": {"value": 7.0}, "f3": {"value": null}},
        {"key": 3_000_000, "doc_count": 1, "f1": {"value": 9.0}, "f3": {"value": 2.0}},
        {"key": 9_000_000, "doc_count": 1, "f1": {"value": 1.0}, "f3": {"value": 1.0}},
    ]}}});
    let got = parse(&answer, &detector(), &SPAN).unwrap();
    assert_eq!(
        got,
        vec![Some(vec![5.0, 1.5]), None, None, Some(vec![9.0, 2.0])],
        "a bucket with no documents, one missing a feature, and one outside the span are missing or ignored"
    );
    assert!(parse(&json!({}), &detector(), &SPAN).is_err());
}

#[test]
fn a_bucket_off_the_interval_is_ignored() {
    let answer = json!({"aggregations": {"windows": {"buckets": [
        {"key": 1_300_000, "doc_count": 3, "f1": {"value": 5.0}, "f3": {"value": 1.5}},
    ]}}});
    assert_eq!(parse(&answer, &detector(), &SPAN).unwrap(), vec![None; 4]);
}

fn hc_detector() -> Value {
    let mut d = detector();
    d["category_field"] = json!(["host", "az"]);
    d
}

#[test]
fn a_category_field_is_a_terms_level_above_the_features() {
    let body = request(&hc_detector(), &SPAN).unwrap();
    let outer = &body["aggs"]["windows"]["aggs"]["entity0"];
    assert_eq!(outer["terms"], json!({"field": "host", "size": 5}));
    let inner = &outer["aggs"]["entity1"];
    assert_eq!(inner["terms"], json!({"field": "az", "size": 5}));
    assert_eq!(inner["aggs"]["f1"], json!({"max": {"field": "cpu"}}));
}

fn leaf(host: &str, az: &str, docs: u64, cpu: f64) -> Value {
    json!({"key": host, "doc_count": docs, "entity1": {"buckets": [
        {"key": az, "doc_count": docs, "f1": {"value": cpu}, "f3": {"value": 1.0}}
    ]}})
}

#[test]
fn the_entities_with_the_most_documents_are_kept_with_their_own_windows() {
    // seven hosts over two windows; host h0..h6 has h_i + 1 documents a window
    let window = |key: i64, upto: u64| {
        let hosts: Vec<Value> = (0..upto)
            .map(|i| leaf(&format!("h{i}"), "z", i + 1, i as f64))
            .collect();
        json!({"key": key, "doc_count": 1, "entity0": {"buckets": hosts}})
    };
    let answer = json!({"aggregations": {"windows": {"buckets": [
        window(1_200_000, 7),
        window(1_800_000, 3),
        window(99_000_000, 7),
    ]}}});
    let got = super::parse_entities(&answer, &hc_detector(), &SPAN).unwrap();
    let keys: Vec<String> = got.iter().map(|e| e.key[0].1.clone()).collect();
    assert_eq!(
        keys,
        ["h6", "h2", "h5", "h4", "h1"],
        "the five with most documents, most first, ties by key"
    );
    assert_eq!(
        got[0].key,
        [
            ("host".to_owned(), "h6".to_owned()),
            ("az".to_owned(), "z".to_owned())
        ]
    );
    assert_eq!(got[0].docs, 7, "the window outside the span is not counted");
    assert_eq!(got[0].windows[0], Some(vec![6.0, 1.0]));
    assert_eq!(
        got[0].windows[1], None,
        "h6 is not among the three hosts of the second window"
    );
    assert_eq!(
        got[1].docs,
        3 + 3,
        "h2 holds three documents in each of two windows"
    );
    assert_eq!(got[1].windows[1], Some(vec![2.0, 1.0]));
    assert_eq!(got[4].docs, 2 + 2);
}

#[test]
fn an_answer_with_no_entities_has_none() {
    let answer = json!({"aggregations": {"windows": {"buckets": [
        {"key": 1_200_000, "doc_count": 0, "entity0": {"buckets": []}},
    ]}}});
    assert!(
        super::parse_entities(&answer, &hc_detector(), &SPAN)
            .unwrap()
            .is_empty()
    );
    assert!(super::parse_entities(&json!({}), &hc_detector(), &SPAN).is_err());
}

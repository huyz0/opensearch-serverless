//! docs/specs/anomaly-detection slice 6b: `_topAnomalies` groups the
//! anomalous results of a range by entity and refuses as the stock plugin
//! does.

use std::sync::{Arc, Mutex};

use oss_node::ad_route::WindowSearch;
use oss_rest::RestError;
use serde_json::{Value, json};

use super::ad_routes::{ROOT, Rig, call, create, detector, json_of, rig_with};

/// Answers every search with `hits` and remembers the queries.
struct Hits {
    hits: Vec<Value>,
    asked: Mutex<Vec<Value>>,
}

impl WindowSearch for Hits {
    fn search(&self, _indices: &[String], body: &Value) -> Result<Value, RestError> {
        self.asked.lock().unwrap().push(body.clone());
        Ok(json!({"hits": {"hits": self.hits}}))
    }
}

fn hit(host: &str, grade: f64) -> Value {
    json!({"_source": {"anomaly_grade": grade, "entity": [{"name": "host", "value": host}]}})
}

fn rig(hits: Vec<Value>) -> (Rig, Arc<Hits>) {
    let search = Arc::new(Hits {
        hits,
        asked: Mutex::new(Vec::new()),
    });
    (rig_with(Some(Arc::clone(&search) as _)), search)
}

fn categorised(rig: &Rig) -> String {
    let mut d = detector();
    d["category_field"] = json!(["host"]);
    create(rig, &d)
}

fn top(rig: &Rig, id: &str, query: &str, body: &Value) -> (u16, Value) {
    let got = call(
        rig,
        "POST",
        &format!("{ROOT}/detectors/{id}/results/_topAnomalies{query}"),
        Some(body),
    );
    (got.status, json_of(&got))
}

fn range() -> Value {
    json!({"start_time_ms": 1_000, "end_time_ms": 2_000})
}

fn with(extra: Value) -> Value {
    let mut b = range();
    for (k, v) in extra.as_object().unwrap() {
        b[k] = v.clone();
    }
    b
}

fn reason(answer: &Value) -> String {
    answer["error"]["reason"].as_str().unwrap_or("").to_owned()
}

#[test]
fn entities_are_ranked_by_severity_then_by_occurrence() {
    let (rig, search) = rig(vec![
        hit("a", 0.2),
        hit("a", 0.3),
        hit("a", 0.1),
        hit("b", 0.9),
        hit("c", 0.5),
        hit("c", 0.5),
    ]);
    let id = categorised(&rig);
    let (status, got) = top(&rig, &id, "", &range());
    assert_eq!(status, 200, "{got}");
    let keys: Vec<&str> = got["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["key"]["host"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["b", "c", "a"]);
    assert_eq!(got["buckets"][2]["doc_count"], 3);
    assert_eq!(got["buckets"][2]["max_anomaly_grade"], 0.3);

    let (_, got) = top(&rig, &id, "", &with(json!({"order": "occurrence"})));
    let keys: Vec<&str> = got["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["key"]["host"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["a", "c", "b"]);

    let (_, got) = top(&rig, &id, "", &with(json!({"size": "1"})));
    assert_eq!(got["buckets"].as_array().unwrap().len(), 1);

    let asked = search.asked.lock().unwrap();
    let filter = asked[0]["query"]["bool"]["filter"].to_string();
    assert!(filter.contains("\"detector_id\""), "{filter}");
    assert!(filter.contains("\"gte\":1000"), "{filter}");
    assert!(filter.contains("must_not"), "{filter}");
}

#[test]
fn a_range_with_no_results_has_no_buckets() {
    let (rig, _) = rig(vec![]);
    let id = categorised(&rig);
    let got = call(
        &rig,
        "GET",
        &format!("{ROOT}/detectors/{id}/results/_topAnomalies"),
        Some(&range()),
    );
    assert_eq!(got.status, 200, "{}", got.text());
    assert_eq!(json_of(&got), json!({"buckets": []}));
}

#[test]
fn bad_input_is_refused_with_the_recorded_messages() {
    let (rig, search) = rig(vec![]);
    let id = categorised(&rig);
    let cases = [
        (
            json!({"start_time_ms": 1}),
            400,
            "Validation Failed: 1: Must set both start time and end time with epoch of milliseconds;",
        ),
        (
            json!({"start_time_ms": 5, "end_time_ms": 5}),
            400,
            "Validation Failed: 1: Start time should be before end time;",
        ),
        (
            with(json!({"start_time_ms": "x"})),
            400,
            "For input string: \"x\"",
        ),
        (
            with(json!({"size": 0})),
            400,
            "Size must be a positive integer",
        ),
        (with(json!({"size": 1001})), 400, "Size cannot exceed 1000"),
        (with(json!({"size": "x"})), 400, "For input string: \"x\""),
        (
            with(json!({"order": "color"})),
            400,
            "Ordering by color is not a valid option",
        ),
        (
            with(json!({"category_field": ["zone"]})),
            400,
            &format!("Category field zone doesn't exist for detector ID {id}"),
        ),
    ];
    for (body, want, text) in cases {
        let (status, got) = top(&rig, &id, "", &body);
        assert_eq!(status, want, "{body}: {got}");
        assert!(reason(&got).contains(text), "{body}: {got}");
    }
    let (status, got) = top(&rig, &id, "?historical=maybe", &range());
    assert_eq!(status, 400);
    assert!(reason(&got).contains("only [true] or [false]"), "{got}");
    let (status, got) = top(&rig, &id, "?historical=true", &range());
    assert_eq!(status, 500, "{got}");
    assert!(reason(&got).contains("No historical tasks found"), "{got}");
    assert!(search.asked.lock().unwrap().is_empty());
}

#[test]
fn missing_detectors_and_missing_categories_and_bodies_are_refused() {
    let (rig, _) = rig(vec![]);
    let (status, got) = top(&rig, "nope", "", &range());
    assert_eq!(status, 404, "{got}");
    assert!(reason(&got).contains("Can't find config with id: nope"));

    let plain = create(&rig, &detector());
    let (status, got) = top(&rig, &plain, "", &range());
    assert_eq!(status, 400, "{got}");
    assert!(reason(&got).contains("No category fields found"), "{got}");

    let id = categorised(&rig);
    let empty = call(
        &rig,
        "POST",
        &format!("{ROOT}/detectors/{id}/results/_topAnomalies"),
        None,
    );
    assert_eq!(empty.status, 400);
    assert!(empty.text().contains("request body is required"));
}

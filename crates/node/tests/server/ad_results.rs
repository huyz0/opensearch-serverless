//! docs/specs/anomaly-detection slice 5: `POST /detectors/results/_search`
//! reads the result index through the node's search.

use std::sync::{Arc, Mutex};

use oss_node::ad_route::WindowSearch;
use oss_rest::RestError;
use serde_json::{Value, json};

use super::ad_routes::{ROOT, call, json_of, rig_with};
use crate::support;

/// Answers a search with `answer`, or with the error when it is `Err`, and
/// remembers what it was asked.
struct Canned {
    asked: Mutex<Vec<(Vec<String>, Value)>>,
    answer: Result<Value, u16>,
}

impl WindowSearch for Canned {
    fn search(&self, indices: &[String], body: &Value) -> Result<Value, RestError> {
        self.asked
            .lock()
            .unwrap()
            .push((indices.to_vec(), body.clone()));
        self.answer.clone().map_err(|status| {
            RestError::exception(
                status,
                oss_rest::Exception::new("index_not_found_exception", "no such index"),
            )
        })
    }
}

fn canned(answer: Result<Value, u16>) -> Arc<Canned> {
    Arc::new(Canned {
        asked: Mutex::new(Vec::new()),
        answer,
    })
}

#[test]
fn a_results_search_reads_the_default_result_index_with_the_callers_body() {
    let hits =
        json!({"hits": {"total": {"value": 1}, "hits": [{"_source": {"anomaly_grade": 0.5}}]}});
    let search = canned(Ok(hits.clone()));
    let rig = rig_with(Some(Arc::clone(&search) as _));
    let query = json!({"query": {"range": {"anomaly_grade": {"gt": 0}}}});
    let got = call(
        &rig,
        "POST",
        &format!("{ROOT}/detectors/results/_search"),
        Some(&query),
    );
    assert_eq!(got.status, 200, "{}", got.text());
    assert_eq!(json_of(&got), hits);
    let asked = search.asked.lock().unwrap();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].0, ["opensearch-ad-plugin-result"]);
    assert_eq!(asked[0].1, query);
}

#[test]
fn a_named_result_index_is_the_one_searched_and_an_empty_body_matches_all() {
    let search = canned(Ok(json!({"hits": {"hits": []}})));
    let rig = rig_with(Some(Arc::clone(&search) as _));
    let got = call(
        &rig,
        "GET",
        &format!("{ROOT}/detectors/results/_search/custom-results"),
        None,
    );
    assert_eq!(got.status, 200, "{}", got.text());
    let asked = search.asked.lock().unwrap();
    assert_eq!(asked[0].0, ["custom-results"]);
    assert_eq!(asked[0].1, json!({}));
}

#[test]
fn a_result_index_no_run_has_written_is_an_empty_answer_not_an_error() {
    let rig = rig_with(Some(canned(Err(404)) as _));
    let got = call(
        &rig,
        "POST",
        &format!("{ROOT}/detectors/results/_search"),
        Some(&json!({})),
    );
    assert_eq!(got.status, 200, "{}", got.text());
    let body = json_of(&got);
    assert_eq!(body["hits"]["total"]["value"], 0);
    assert_eq!(body["hits"]["hits"], json!([]));
}

#[test]
fn another_search_failure_and_a_bad_body_and_no_search_are_refused() {
    let rig = rig_with(Some(canned(Err(429)) as _));
    let url = format!("{ROOT}/detectors/results/_search");
    assert_eq!(call(&rig, "POST", &url, Some(&json!({}))).status, 429);
    let ok = rig_with(Some(canned(Ok(json!({}))) as _));
    let bad = support::request(ok.server.local_addr(), "POST", &url, Some(b"{not json"));
    assert_eq!(bad.status, 400);
    let none = rig_with(None);
    assert_eq!(call(&none, "POST", &url, Some(&json!({}))).status, 500);
}

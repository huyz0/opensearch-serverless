//! docs/specs/anomaly-detection slice 3: `_preview` of a detector.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oss_anomaly::{Model, Params, Workspace};
use oss_blob::{BlobStore, S3Op};
use oss_node::ad_route::WindowSearch;
use oss_rest::RestError;
use serde_json::{Value, json};

use super::ad_routes::{ROOT, Rig, call, detector, json_of, rig_with, total};

const MINUTE: i64 = 60_000;
const START: i64 = 1_700_000_000_000 - 1_700_000_000_000 % (10 * MINUTE);
/// The window of the range that carries the spike.
const SPIKE: usize = 40;
const WINDOWS: usize = 60;
const STEP: i64 = 10 * MINUTE;
const SPIKE_AT: i64 = START + 40 * STEP;

/// What a window of the series holds at `at`: a repeating ramp, and a spike.
fn value_at(at: i64) -> f64 {
    if at == SPIKE_AT {
        500.0
    } else {
        ((at / (10 * MINUTE)) % 10) as f64
    }
}

/// A search that answers the histogram it is asked for from the series, and
/// counts how often it is asked.
struct Series {
    asked: AtomicUsize,
    /// Windows before this time hold no data.
    data_from: i64,
    empty: bool,
}

impl WindowSearch for Series {
    fn search(&self, indices: &[String], body: &Value) -> Result<Value, RestError> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        assert_eq!(indices, ["metrics"]);
        let h = &body["aggs"]["windows"];
        let min = h["date_histogram"]["extended_bounds"]["min"]
            .as_i64()
            .expect("a start");
        let max = h["date_histogram"]["extended_bounds"]["max"]
            .as_i64()
            .expect("an end");
        let interval: i64 = h["date_histogram"]["fixed_interval"]
            .as_str()
            .and_then(|s| s.strip_suffix("ms"))
            .and_then(|s| s.parse().ok())
            .expect("an interval");
        let feature = h["aggs"]
            .as_object()
            .and_then(|m| m.keys().next())
            .expect("a feature")
            .clone();
        let mut buckets = Vec::new();
        let mut key = min;
        while key <= max {
            let has = !self.empty && key >= self.data_from;
            buckets.push(json!({
                "key": key,
                "doc_count": u64::from(has),
                feature.clone(): {"value": has.then(|| value_at(key))},
            }));
            key = key.saturating_add(interval);
        }
        Ok(json!({"aggregations": {"windows": {"buckets": buckets}}}))
    }
}

fn series() -> Arc<Series> {
    Arc::new(Series {
        asked: AtomicUsize::new(0),
        data_from: 0,
        empty: false,
    })
}

fn rig(search: &Arc<Series>) -> Rig {
    rig_with(Some(Arc::clone(search) as Arc<dyn WindowSearch>))
}

fn small() -> Value {
    let mut d = detector();
    d["indices"] = json!(["metrics"]);
    d["shingle_size"] = json!(4);
    d
}

fn range(windows: usize) -> Value {
    json!({
        "period_start": START,
        "period_end": START.saturating_add(i64::try_from(windows).unwrap().saturating_mul(STEP)),
        "detector": small(),
    })
}

fn preview(r: &Rig, body: &Value) -> support_reply::Reply {
    call(r, "POST", &format!("{ROOT}/detectors/_preview"), Some(body))
}

use crate::support as support_reply;

// AC-7
#[test]
fn a_preview_grades_every_window_with_one_search_and_no_put() {
    let search = series();
    let r = rig(&search);
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    let got = preview(&r, &range(WINDOWS));
    assert_eq!(got.status, 200, "{}", got.text());
    assert_eq!(
        search.asked.load(Ordering::SeqCst),
        1,
        "one search of the reader"
    );
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (0, 0),
        "an inline detector is read from nowhere and nothing is stored"
    );
    assert_eq!(total(&r, S3Op::List), 0);
    let out = json_of(&got);
    let results = out["anomaly_result"].as_array().expect("results");
    assert_eq!(results.len(), WINDOWS);
    assert_eq!(results[0]["data_start_time"], START);
    assert_eq!(results[0]["data_end_time"], START + 10 * MINUTE);
    assert_eq!(out["anomaly_detector"]["shingle_size"], 4);

    // the grades are those of a model fed the same windows directly
    let mut params = Params::new(1, 0x00AD_5EED);
    params.shingle_size = 4;
    let history = params.min_train_samples + params.shingle_size;
    let mut ws = Workspace::new(&params);
    let mut model = Model::new(params).expect("a model");
    let mut direct = Vec::new();
    for slot in 0..history + WINDOWS {
        let at = START + (slot as i64 - history as i64) * 10 * MINUTE;
        let s = model
            .feed(&mut ws, slot as u64, Some(&[value_at(at) as f32]))
            .expect("fed");
        if slot >= history {
            direct.push(s.map_or(0.0, |s| s.grade));
        }
    }
    let grades: Vec<f64> = results
        .iter()
        .map(|r| r["anomaly_grade"].as_f64().unwrap())
        .collect();
    assert_eq!(grades, direct);
    assert!(grades[SPIKE] > 0.0, "the spike is flagged: {grades:?}");
    assert!(
        grades[..SPIKE - 1].iter().all(|g| *g < 0.5),
        "steady windows are not"
    );
}

#[test]
fn a_stored_detector_is_read_for_its_preview_and_a_missing_one_is_404() {
    let search = series();
    let r = rig(&search);
    let made = call(&r, "POST", &format!("{ROOT}/detectors"), Some(&small()));
    let id = json_of(&made)["_id"].as_str().unwrap().to_owned();
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    let mut body = range(20);
    body.as_object_mut().unwrap().remove("detector");
    let got = call(
        &r,
        "POST",
        &format!("{ROOT}/detectors/{id}/_preview"),
        Some(&body),
    );
    assert_eq!(got.status, 200, "{}", got.text());
    assert_eq!(
        json_of(&got)["anomaly_result"].as_array().unwrap().len(),
        20
    );
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (2, 0),
        "the detector's marker and content, nothing written"
    );
    let missing = call(
        &r,
        "POST",
        &format!("{ROOT}/detectors/nowhere/_preview"),
        Some(&body),
    );
    assert_eq!(missing.status, 404, "{}", missing.text());
}

#[test]
fn a_range_wider_than_the_bound_or_badly_made_is_refused_before_any_search() {
    let search = series();
    let r = rig(&search);
    let wide = preview(&r, &range(1_001));
    assert_eq!(wide.status, 400, "{}", wide.text());
    assert!(wide.text().contains("1 to 1000"), "{}", wide.text());
    let mut backwards = range(10);
    backwards["period_end"] = json!(START - 1);
    assert_eq!(preview(&r, &backwards).status, 400);
    let mut no_start = range(10);
    no_start.as_object_mut().unwrap().remove("period_start");
    let refused = preview(&r, &no_start);
    assert_eq!(refused.status, 400);
    assert!(
        refused.text().contains("period_start"),
        "{}",
        refused.text()
    );
    let mut no_detector = range(10);
    no_detector.as_object_mut().unwrap().remove("detector");
    assert_eq!(preview(&r, &no_detector).status, 400);
    let mut invalid = range(10);
    invalid["detector"]["shingle_size"] = json!(61);
    assert_eq!(preview(&r, &invalid).status, 400);
    let mut hc = range(10);
    hc["detector"]["category_field"] = json!(["host"]);
    let hc = preview(&r, &hc);
    assert_eq!(hc.status, 400);
    assert!(hc.text().contains("category_field"), "{}", hc.text());
    assert_eq!(
        search.asked.load(Ordering::SeqCst),
        0,
        "none of them searched"
    );
}

#[test]
fn a_range_with_no_data_answers_an_empty_result_and_a_node_with_no_search_refuses() {
    let empty = Arc::new(Series {
        asked: AtomicUsize::new(0),
        data_from: 0,
        empty: true,
    });
    let r = rig(&empty);
    let got = preview(&r, &range(10));
    assert_eq!(got.status, 200, "{}", got.text());
    assert_eq!(json_of(&got)["anomaly_result"], json!([]));
    let bare = rig_with(None);
    let refused = preview(&bare, &range(10));
    assert_eq!(refused.status, 500, "{}", refused.text());
}

/// A node assembled with the setting on wires its previews to its own
/// search: a detector over an index the node does not hold is the search's
/// 404, not a node that cannot search.
#[test]
fn an_assembled_node_previews_through_its_own_search() {
    let config = oss_node::assemble::Config {
        ad: true,
        ..super::assemble::config()
    };
    let (r, _) = super::assemble::start_with(config, 256 << 20);
    let got = super::assemble::call(
        &r,
        "POST",
        &format!("{ROOT}/detectors/_preview"),
        Some(&range(10).to_string()),
    );
    assert_eq!(got.status, 404, "{}", got.text());
    assert!(got.text().contains("index_not_found"), "{}", got.text());
}

/// A request-body pool too small for the model is refused as full, before
/// any search.
#[test]
fn a_pool_too_small_for_the_model_refuses_the_preview() {
    let config = oss_node::assemble::Config {
        ad: true,
        ..super::assemble::config()
    };
    let (r, store) = super::assemble::start_with(config, 768 << 10);
    let gets = store.meter().count(S3Op::Get);
    let got = super::assemble::call(
        &r,
        "POST",
        &format!("{ROOT}/detectors/_preview"),
        Some(&range(10).to_string()),
    );
    assert_eq!(got.status, 429, "{}", got.text());
    assert_eq!(store.meter().count(S3Op::Get), gets, "nothing was read");
}

// slice 3 follow-up: `_validate` answers what a create would, and writes nothing.
#[test]
fn a_validate_answers_what_a_create_would_and_writes_nothing() {
    let r = rig_with(None);
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    let ok = call(
        &r,
        "POST",
        &format!("{ROOT}/detectors/_validate"),
        Some(&small()),
    );
    assert_eq!(ok.status, 200, "{}", ok.text());
    assert_eq!(json_of(&ok), json!({}));
    let mut bad = small();
    bad["shingle_size"] = json!(61);
    let refused = call(
        &r,
        "POST",
        &format!("{ROOT}/detectors/_validate/detector"),
        Some(&bad),
    );
    assert_eq!(refused.status, 200, "{}", refused.text());
    let out = json_of(&refused);
    assert!(
        out["detector"]["shingle_size"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("1 to 60")),
        "{out}"
    );
    let model = call(
        &r,
        "POST",
        &format!("{ROOT}/detectors/_validate/model"),
        Some(&small()),
    );
    assert_eq!(model.status, 400, "{}", model.text());
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (0, 0),
        "nothing read or written"
    );
}

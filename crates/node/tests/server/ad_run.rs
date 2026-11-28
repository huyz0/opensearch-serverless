//! docs/specs/anomaly-detection slice 4b: one run of a detector over the
//! claim register, the window search, the model and the result sink.

// ARITH: slot times here are small test constants; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::{Arc, Mutex};

use oss_blob::{BlobStore, S3Op};
use oss_node::ad_route::{
    Doc, DocSink, HostError, IndexReport, Outcome, Runner, Timing, WindowSearch,
};
use oss_rest::RestError;
use serde_json::{Value, json};

use super::ad_routes::{ROOT, Rig, call, create, detector, json_of, rig_with, total};

const PERIOD: i64 = 10 * 60_000;
/// The detector's `window_delay`, 1 minute.
const DELAY: i64 = 60_000;
const TIMING: Timing = Timing {
    max_skew_ms: 5_000,
    visibility_lag_ms: 2_000,
};
/// The slot the series spikes in, after the model has seen enough windows.
const SPIKE: u64 = 200;

/// What a window holds: a repeating ramp, and a spike.
fn value_at(slot: u64) -> f64 {
    if slot == SPIKE {
        500.0
    } else {
        (slot % 10) as f64
    }
}

/// A source whose every window holds one document with a value from `value_at`.
struct Series {
    feature: String,
    searches: Mutex<u32>,
}

impl WindowSearch for Series {
    fn search(&self, _indices: &[String], body: &Value) -> Result<Value, RestError> {
        *self.searches.lock().unwrap() += 1;
        let from = body["query"]["bool"]["filter"][0]["range"]["timestamp"]["gte"]
            .as_i64()
            .expect("a range");
        let slot = u64::try_from((from + DELAY) / PERIOD).unwrap();
        let mut bucket = json!({"key": from, "doc_count": 1});
        bucket[self.feature.as_str()] = json!({"value": value_at(slot)});
        Ok(json!({"aggregations": {"windows": {"buckets": [bucket]}}}))
    }
}

#[derive(Default)]
struct Written(Mutex<Vec<(String, Value)>>);

impl DocSink for Written {
    fn index(&self, index: &str, docs: &[Doc]) -> Result<IndexReport, HostError> {
        for d in docs {
            let v: Value = serde_json::from_slice(&d.source).expect("json");
            self.0.lock().unwrap().push((index.to_owned(), v));
        }
        Ok(IndexReport {
            indexed: docs.len(),
            failed: 0,
        })
    }
}

struct Fixture {
    rig: Rig,
    id: String,
    detector: Value,
    feature: String,
}

fn fixture() -> Fixture {
    let rig = rig_with(None);
    let id = create(&rig, &detector());
    let got = json_of(&call(&rig, "GET", &format!("{ROOT}/detectors/{id}"), None));
    let stored = got["anomaly_detector"].clone();
    let feature = stored["feature_attributes"][0]["feature_id"]
        .as_str()
        .expect("a feature id")
        .to_owned();
    Fixture {
        rig,
        id,
        detector: stored,
        feature,
    }
}

fn runner(f: &Fixture, sink: &Arc<Written>) -> (Runner, Arc<Series>) {
    let search = Arc::new(Series {
        feature: f.feature.clone(),
        searches: Mutex::new(0),
    });
    let store: Arc<dyn BlobStore> = Arc::clone(&f.rig.store) as Arc<dyn BlobStore>;
    let run = Runner::new(
        &store,
        "",
        Arc::clone(&search) as Arc<dyn WindowSearch>,
        Arc::clone(sink) as Arc<dyn DocSink>,
        TIMING,
    )
    .expect("a runner");
    (run, search)
}

/// The time a slot is first due.
fn due(slot: u64) -> i64 {
    (i64::try_from(slot).unwrap() + 1) * PERIOD + TIMING.max_skew_ms + TIMING.visibility_lag_ms
}

#[test]
fn nothing_is_read_or_written_before_a_slot_is_due() {
    let f = fixture();
    let sink = Arc::new(Written::default());
    let (run, search) = runner(&f, &sink);
    let before = (total(&f.rig, S3Op::Get), total(&f.rig, S3Op::Put));
    assert_eq!(
        run.run(&f.id, &f.detector, due(0) - 1).unwrap(),
        Outcome::NotDue
    );
    assert_eq!(
        (total(&f.rig, S3Op::Get), total(&f.rig, S3Op::Put)),
        before,
        "an undue run makes no request"
    );
    assert_eq!(*search.searches.lock().unwrap(), 0);
}

#[test]
fn a_slot_is_claimed_once_and_costs_a_claim_and_one_search() {
    let f = fixture();
    let sink = Arc::new(Written::default());
    let (run, search) = runner(&f, &sink);
    let (gets, puts) = (total(&f.rig, S3Op::Get), total(&f.rig, S3Op::Put));
    let first = run.run(&f.id, &f.detector, due(0)).unwrap();
    assert_eq!(
        first,
        Outcome::Filling { slot: 0 },
        "a new model scores nothing yet"
    );
    assert_eq!(
        (
            total(&f.rig, S3Op::Get) - gets,
            total(&f.rig, S3Op::Put) - puts
        ),
        (1, 1),
        "a first claim reads the register and creates it"
    );
    assert_eq!(*search.searches.lock().unwrap(), 1);

    let again = run.run(&f.id, &f.detector, due(0) + 1_000).unwrap();
    assert_eq!(again, Outcome::Lost { slot: 0 }, "a slot is run once");
    assert_eq!(
        *search.searches.lock().unwrap(),
        1,
        "a lost slot is not searched"
    );

    let (gets, puts) = (total(&f.rig, S3Op::Get), total(&f.rig, S3Op::Put));
    assert_eq!(
        run.run(&f.id, &f.detector, due(1)).unwrap(),
        Outcome::Filling { slot: 1 }
    );
    assert_eq!(
        total(&f.rig, S3Op::Put) - puts,
        1,
        "a hinted claim is one swap"
    );
    assert_eq!(total(&f.rig, S3Op::Get), gets, "and reads nothing");
    assert_eq!(total(&f.rig, S3Op::List), 0);
}

#[test]
fn a_spike_after_enough_windows_is_scored_and_written() {
    let f = fixture();
    let sink = Arc::new(Written::default());
    let (run, _) = runner(&f, &sink);
    let mut last = None;
    for slot in 0..=SPIKE {
        last = Some(run.run(&f.id, &f.detector, due(slot)).unwrap());
    }
    let Some(Outcome::Scored { slot, grade }) = last else {
        panic!("the spike was not scored: {last:?}");
    };
    assert_eq!(slot, SPIKE);
    assert!(grade > 0.0, "a spike has a grade, got {grade}");
    let written = sink.0.lock().unwrap();
    let (index, doc) = written.last().expect("a result");
    assert_eq!(index, "opensearch-ad-plugin-result");
    assert_eq!(doc["detector_id"], f.id.as_str());
    assert_eq!(doc["anomaly_grade"], grade);
    let from = i64::try_from(SPIKE).unwrap() * PERIOD - DELAY;
    assert_eq!(
        doc["data_start_time"], from,
        "the window less the 1 minute delay"
    );
    assert_eq!(doc["data_end_time"], from + PERIOD);
    assert_eq!(doc["feature_data"][0]["data"], 500.0);
    assert!(
        written
            .iter()
            .all(|(_, d)| d["anomaly_grade"].as_f64().is_some()),
        "every document has a grade"
    );
}

#[test]
fn another_nodes_claim_drops_the_model_so_two_lineages_never_feed_one_detector() {
    let f = fixture();
    let sink = Arc::new(Written::default());
    let (a, _) = runner(&f, &sink);
    let (b, _) = runner(&f, &sink);
    for slot in 0..SPIKE {
        a.run(&f.id, &f.detector, due(slot)).unwrap();
    }
    assert_eq!(a.held(), 1);
    // b takes a slot, as it would after a takeover, with a model of its own
    assert_eq!(
        b.run(&f.id, &f.detector, due(SPIKE)).unwrap(),
        Outcome::Filling { slot: SPIKE },
        "a node with no model scores nothing yet"
    );
    // a's next claim finds the register moved: its model is dropped, not fed
    let next = a.run(&f.id, &f.detector, due(SPIKE + 1)).unwrap();
    assert_eq!(next, Outcome::Filling { slot: SPIKE + 1 }, "{next:?}");
}

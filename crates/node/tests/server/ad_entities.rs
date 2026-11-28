//! docs/specs/anomaly-detection slice 5: a detector with category fields
//! keeps a model for each entity and writes each one's result with its
//! entity.
#![allow(clippy::arithmetic_side_effects)] // ARITH: slot and time arithmetic in a test

use std::sync::{Arc, Mutex};

use oss_blob::BlobStore;
use oss_ext::doc_sink::{Doc, DocResult, DocSink};
use oss_ext::host::HostError;
use oss_node::ad_route::{Outcome, Runner, Timing, WindowSearch};
use oss_rest::RestError;
use serde_json::{Value, json};

use super::ad_routes::{ROOT, Rig, call, create, detector, json_of, rig_with};

const PERIOD: i64 = 10 * 60_000;
const DELAY: i64 = 60_000;
const TIMING: Timing = Timing {
    max_skew_ms: 5_000,
    visibility_lag_ms: 2_000,
};
const SPIKE: u64 = 200;

/// A source of `hosts` entities, `host-0` first; every window of every one
/// holds a ramp, and `host-0` spikes at `SPIKE`.
struct Hosts {
    features: Mutex<Vec<String>>,
    hosts: Mutex<usize>,
    asked: Mutex<Vec<Value>>,
}

impl WindowSearch for Hosts {
    fn search(&self, _indices: &[String], body: &Value) -> Result<Value, RestError> {
        self.asked.lock().unwrap().push(body.clone());
        let from = body["query"]["bool"]["filter"][0]["range"]["timestamp"]["gte"]
            .as_i64()
            .expect("a range");
        let slot = u64::try_from((from + DELAY) / PERIOD).unwrap();
        let buckets: Vec<Value> = (0..*self.hosts.lock().unwrap())
            .map(|h| {
                let value = if h == 0 && slot == SPIKE {
                    500.0
                } else {
                    (slot % 10) as f64
                };
                let mut b = json!({"key": format!("host-{h}"), "doc_count": 1});
                for feature in self.features.lock().unwrap().iter() {
                    b[feature.as_str()] = json!({"value": value});
                }
                b
            })
            .collect();
        Ok(json!({"aggregations": {"windows": {"buckets": [
            {"key": from, "doc_count": 1, "entity0": {"buckets": buckets}}
        ]}}}))
    }
}

#[derive(Default)]
struct Written(Mutex<Vec<Value>>);

impl DocSink for Written {
    fn index(&self, _index: &str, docs: &[Doc]) -> Result<Vec<DocResult>, HostError> {
        for d in docs {
            self.0
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&d.source).expect("json"));
        }
        Ok(docs
            .iter()
            .map(|_| DocResult {
                id: String::new(),
                status: 201,
            })
            .collect())
    }
}

struct Fixture {
    rig: Rig,
}

impl Fixture {
    fn detector(&self, name: &str) -> (String, Value, String) {
        let mut d = detector();
        d["name"] = json!(name);
        d["category_field"] = json!(["host"]);
        let id = create(&self.rig, &d);
        let got = json_of(&call(
            &self.rig,
            "GET",
            &format!("{ROOT}/detectors/{id}"),
            None,
        ));
        let stored = got["anomaly_detector"].clone();
        let feature = stored["feature_attributes"][0]["feature_id"]
            .as_str()
            .unwrap()
            .to_owned();
        (id, stored, feature)
    }

    fn runner(&self, feature: &str, hosts: usize) -> (Runner, Arc<Hosts>, Arc<Written>) {
        let search = Arc::new(Hosts {
            features: Mutex::new(vec![feature.to_owned()]),
            hosts: Mutex::new(hosts),
            asked: Mutex::new(Vec::new()),
        });
        let sink = Arc::new(Written::default());
        let store: Arc<dyn BlobStore> = Arc::clone(&self.rig.store) as Arc<dyn BlobStore>;
        let run = Runner::new(
            &store,
            "",
            Arc::clone(&search) as Arc<dyn WindowSearch>,
            Arc::clone(&sink) as Arc<dyn DocSink>,
            TIMING,
        )
        .expect("a runner");
        (run, search, sink)
    }
}

fn due(slot: u64) -> i64 {
    (i64::try_from(slot).unwrap() + 1) * PERIOD + TIMING.max_skew_ms + TIMING.visibility_lag_ms
}

#[test]
fn each_entity_has_its_own_model_and_its_results_name_it() {
    let f = Fixture {
        rig: rig_with(None),
    };
    let (id, stored, feature) = f.detector("by-host");
    let (run, search, sink) = f.runner(&feature, 2);
    let mut last = Outcome::NotDue;
    for slot in 0..=SPIKE {
        last = run.run(&id, &stored, due(slot)).unwrap();
    }
    assert_eq!(
        last,
        Outcome::Entities {
            slot: SPIKE,
            scored: 2,
            skipped: 0
        }
    );
    assert_eq!(run.models(), 2, "one model for each host");
    let asked = search.asked.lock().unwrap();
    assert_eq!(
        asked[0]["aggs"]["windows"]["aggs"]["entity0"]["terms"]["size"], 100,
        "a run reads up to 100 entities"
    );
    let written = sink.0.lock().unwrap();
    let spike: Vec<&Value> = written
        .iter()
        .filter(|d| d["data_start_time"] == i64::try_from(SPIKE).unwrap() * PERIOD - DELAY)
        .collect();
    assert_eq!(spike.len(), 2);
    let named = |host: &str| {
        spike
            .iter()
            .find(|d| d["entity"] == json!([{"name": "host", "value": host}]))
            .expect("a result for the host")
    };
    assert!(named("host-0")["anomaly_grade"].as_f64().unwrap() > 0.0);
    assert_eq!(named("host-1")["anomaly_grade"], 0.0);
}

#[test]
fn a_node_at_its_model_cap_leaves_new_entities_out_and_says_so() {
    let f = Fixture {
        rig: rig_with(None),
    };
    let mut outcomes = Vec::new();
    // 100 entities a detector: three fill the cap of 300, the fourth gets none
    let (first, stored, feature) = f.detector("d0");
    let (run, search, _) = f.runner(&feature, 100);
    outcomes.push(run.run(&first, &stored, due(0)).unwrap());
    for n in 1..4 {
        let (id, stored, feature) = f.detector(&format!("d{n}"));
        search.features.lock().unwrap().push(feature);
        outcomes.push(run.run(&id, &stored, due(0)).unwrap());
    }
    assert_eq!(run.models(), 300);
    assert_eq!(
        outcomes[3],
        Outcome::Entities {
            slot: 0,
            scored: 0,
            skipped: 100
        }
    );
}

#[test]
fn an_entity_unseen_for_longer_than_the_models_gap_is_dropped() {
    let f = Fixture {
        rig: rig_with(None),
    };
    let (id, stored, feature) = f.detector("by-host");
    let (run, search, _) = f.runner(&feature, 1);
    run.run(&id, &stored, due(0)).unwrap();
    assert_eq!(run.models(), 1);
    // two windows without the host are within the gap of 2; the third is not
    *search.hosts.lock().unwrap() = 0;
    run.run(&id, &stored, due(2)).unwrap();
    assert_eq!(run.models(), 1);
    run.run(&id, &stored, due(3)).unwrap();
    assert_eq!(run.models(), 0, "unseen for 3 windows");
}

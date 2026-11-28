//! docs/specs/anomaly-detection slice 4b: one run of a detector over the
//! claim register, the window search, the model and the result sink.

// ARITH: slot times here are small test constants; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::{Arc, Mutex};

use oss_blob::{BlobStore, S3Op};
use oss_ext::doc_sink::{Doc, DocResult, DocSink};
use oss_ext::host::HostError;
use oss_node::ad_route::{Outcome, Runner, Scheduler, Timing, WindowSearch};
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
    fn index(&self, index: &str, docs: &[Doc]) -> Result<Vec<DocResult>, HostError> {
        for d in docs {
            let v: Value = serde_json::from_slice(&d.source).expect("json");
            self.0.lock().unwrap().push((index.to_owned(), v));
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
    assert_eq!(again, Outcome::NotDue, "a slot is run once");
    assert_eq!(
        *search.searches.lock().unwrap(),
        1,
        "a slot already run is not searched"
    );

    let (other, other_search) = runner(&f, &sink);
    assert_eq!(
        other.run(&f.id, &f.detector, due(0) + 1_000).unwrap(),
        Outcome::Lost { slot: 0 },
        "another node loses a claimed slot"
    );
    assert_eq!(*other_search.searches.lock().unwrap(), 0);

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
    let (a, a_search) = runner(&f, &sink);
    let (b, _) = runner(&f, &sink);
    for slot in 0..SPIKE {
        a.run(&f.id, &f.detector, due(slot)).unwrap();
    }
    assert_eq!(a.held(), 1);
    // b takes a slot, as it would after a takeover, with a model of its own
    // that is warmed from the windows before it, so it scores at once
    assert!(matches!(
        b.run(&f.id, &f.detector, due(SPIKE)).unwrap(),
        Outcome::Scored { slot: SPIKE, .. }
    ));
    // a's next claim finds the register moved: its model is dropped, not fed,
    // and a new one costs a search for its history besides the window's
    let before = *a_search.searches.lock().unwrap();
    let next = a.run(&f.id, &f.detector, due(SPIKE + 1)).unwrap();
    assert!(matches!(next, Outcome::Scored { .. }), "{next:?}");
    assert_eq!(*a_search.searches.lock().unwrap() - before, 2);
}

#[test]
fn a_new_model_is_warmed_with_one_search_for_its_history_and_scores_at_once() {
    let f = fixture();
    let sink = Arc::new(Written::default());
    let (run, search) = runner(&f, &sink);
    let first = run.run(&f.id, &f.detector, due(SPIKE)).unwrap();
    assert!(
        matches!(first, Outcome::Scored { slot: SPIKE, .. }),
        "{first:?}"
    );
    assert_eq!(
        *search.searches.lock().unwrap(),
        2,
        "history, then the window"
    );
    let again = run.run(&f.id, &f.detector, due(SPIKE + 1)).unwrap();
    assert!(matches!(again, Outcome::Scored { .. }));
    assert_eq!(
        *search.searches.lock().unwrap(),
        3,
        "a held model needs the window only"
    );
}

/// A scheduler for node `name` over the fixture's store.
fn scheduler(f: &Fixture, sink: &Arc<Written>, name: &str) -> Scheduler {
    let (run, _) = runner(f, sink);
    let store: Arc<dyn BlobStore> = Arc::clone(&f.rig.store) as Arc<dyn BlobStore>;
    Scheduler::new(&store, "", run, name, 30_000, 1 << 16).expect("a scheduler")
}

fn start(f: &Fixture) {
    let started = call(
        &f.rig,
        "POST",
        &format!("{ROOT}/detectors/{}/_start", f.id),
        None,
    );
    assert_eq!(started.status, 200, "{}", started.text());
}

const NODES: [&str; 3] = ["a", "b", "c"];
/// A fleet of one: its node owns every detector.
const ALONE: [&str; 1] = ["a"];

#[test]
fn three_nodes_run_a_detector_once_a_slot_and_only_its_owner_runs_it() {
    let f = fixture();
    let sink = Arc::new(Written::default());
    let nodes: Vec<Scheduler> = NODES.iter().map(|n| scheduler(&f, &sink, n)).collect();
    start(&f);
    let mut ran: Vec<(usize, u64)> = Vec::new();
    for slot in 0..6u64 {
        for (i, node) in nodes.iter().enumerate() {
            let report = node.tick(due(slot), &NODES).unwrap();
            assert!(report.failed.is_empty(), "{:?}", report.failed);
            for (id, outcome) in report.outcomes {
                assert_eq!(id, f.id);
                assert_eq!(outcome, Outcome::Filling { slot });
                ran.push((i, slot));
            }
        }
    }
    let slots: Vec<u64> = ran.iter().map(|(_, s)| *s).collect();
    assert_eq!(slots, [0, 1, 2, 3, 4, 5], "each slot ran once");
    assert!(
        ran.iter().all(|(i, _)| *i == ran[0].0),
        "one owner ran them all: {ran:?}"
    );
}

#[test]
fn between_refreshes_a_tick_with_nothing_due_makes_no_request() {
    let f = fixture();
    let sink = Arc::new(Written::default());
    let node = scheduler(&f, &sink, "a");
    start(&f);
    let t0 = due(0);
    node.tick(t0, &ALONE).unwrap();
    let (gets, puts) = (total(&f.rig, S3Op::Get), total(&f.rig, S3Op::Put));
    for step in 1..=20 {
        let report = node.tick(t0 + step * 1_000, &ALONE).unwrap();
        assert!(!report.refreshed && report.outcomes.is_empty());
    }
    assert_eq!(
        (total(&f.rig, S3Op::Get), total(&f.rig, S3Op::Put)),
        (gets, puts),
        "nothing is due and no refresh is: nothing is requested"
    );
    // a refresh reads the set's marker, and no detector
    let before = total(&f.rig, S3Op::Get);
    let report = node.tick(t0 + 31_000, &ALONE).unwrap();
    assert!(report.refreshed);
    assert_eq!(total(&f.rig, S3Op::Get) - before, 1);
    assert_eq!(total(&f.rig, S3Op::List), 0);
}

#[test]
fn a_cluster_with_a_created_detector_reads_the_set_once_and_the_detector_once() {
    let f = fixture();
    let sink = Arc::new(Written::default());
    let node = scheduler(&f, &sink, "a");
    let before = total(&f.rig, S3Op::Get);
    let report = node.tick(due(0), &ALONE).unwrap();
    assert!(report.refreshed && report.outcomes.is_empty());
    // the marker, the names, and the one detector's two reads
    assert_eq!(total(&f.rig, S3Op::Get) - before, 4);
}

#[test]
fn when_the_owner_leaves_another_node_runs_the_detector_with_a_new_model() {
    let f = fixture();
    let sink = Arc::new(Written::default());
    let nodes: Vec<Scheduler> = NODES.iter().map(|n| scheduler(&f, &sink, n)).collect();
    start(&f);
    let owner = (0..3)
        .find(|i| {
            nodes[*i]
                .tick(due(0), &NODES)
                .is_ok_and(|r| !r.outcomes.is_empty())
        })
        .expect("one node owns it");
    let rest: Vec<&str> = NODES
        .iter()
        .copied()
        .filter(|n| *n != NODES[owner])
        .collect();
    let mut took = None;
    for (i, node) in nodes.iter().enumerate().filter(|(i, _)| *i != owner) {
        let report = node.tick(due(1), &rest).unwrap();
        if let Some((_, outcome)) = report.outcomes.first() {
            took = Some((i, outcome.clone()));
        }
    }
    let (new_owner, outcome) = took.expect("another node took the detector");
    assert_ne!(new_owner, owner);
    assert_eq!(
        outcome,
        Outcome::Filling { slot: 1 },
        "the new owner starts a model"
    );
}

#[test]
fn a_stopped_detector_costs_nothing_after_the_next_refresh() {
    let f = fixture();
    let sink = Arc::new(Written::default());
    let node = scheduler(&f, &sink, "a");
    start(&f);
    assert_eq!(node.tick(due(0), &ALONE).unwrap().outcomes.len(), 1);
    let stopped = call(
        &f.rig,
        "POST",
        &format!("{ROOT}/detectors/{}/_stop", f.id),
        None,
    );
    assert_eq!(stopped.status, 200);
    let report = node.tick(due(1), &ALONE).unwrap();
    assert!(report.refreshed, "the refresh interval has passed");
    assert!(
        report.outcomes.is_empty(),
        "the stopped detector does not run"
    );
}

//! The routes' logic against the stock recording in fixtures/async-search
//! (docs/specs/async-search AC-1 to AC-6, AC-11).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use serde_json::{Value, json};

use super::*;
use crate::jobs::{Clock, Failure, Limits};

#[derive(Default)]
struct Gate(Mutex<bool>, Condvar);

impl Gate {
    fn open(&self) {
        *self.0.lock().expect("gate") = true;
        self.1.notify_all();
    }

    fn wait(&self) {
        let mut open = self.0.lock().expect("gate");
        while !*open {
            open = self.1.wait(open).expect("gate");
        }
    }
}

/// Plans a search by the body it is given: `{"fail":true}` is refused, a
/// body naming a gate holds the search there, `{"error":true}` fails while
/// running.
struct Fake(Arc<Gate>);

const RESPONSE: &str = r#"{"took":1,"timed_out":false,"hits":{"hits":[]}}"#;
const PARTIAL: &str = r#"{"took":0,"timed_out":false,"num_reduce_phases":0,"_shards":{"total":1,"successful":0,"skipped":0,"failed":0},"hits":{"max_score":null,"hits":[]}}"#;

impl Runner for Fake {
    fn plan(&self, s: &Submit<'_>, _: &[(String, String)]) -> Result<Plan, RestError> {
        let body = String::from_utf8_lossy(s.body).into_owned();
        if body.contains("fail") {
            return Err(RestError::exception(
                400,
                oss_rest::Exception::new("parsing_exception", "unknown query [nope]"),
            ));
        }
        let gate = Arc::clone(&self.0);
        let (held, error) = (body.contains("slow"), body.contains("error"));
        let work: Work = Box::new(move |_| {
            if held {
                gate.wait();
            }
            if error {
                return Err(Failure::Error(
                    r#"{"type":"index_not_found_exception","reason":"no such index [x]"}"#.into(),
                ));
            }
            Ok(RESPONSE.as_bytes().to_vec())
        });
        Ok(Plan {
            work,
            partial: PARTIAL.into(),
        })
    }
}

fn rig() -> (Service, Arc<Gate>) {
    let config = BudgetConfig::new(256 << 20)
        .with_pool(Pool::Extensions, 128 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let now = Arc::new(AtomicU64::new(1_000));
    let clock: Clock = Arc::new(move || now.load(Ordering::Relaxed));
    let gate = Arc::new(Gate::default());
    let limits = Limits {
        max_running_per_tenant: 3,
        ..Limits::default()
    };
    let jobs = Jobs::new("n1", &budget, limits, clock);
    (
        Service::new(jobs, Arc::new(Fake(Arc::clone(&gate))), "opensearch", "n1"),
        gate,
    )
}

fn who() -> Who {
    Who {
        tenant: "t".into(),
        user: "u".into(),
    }
}

fn q(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

const PATH: &str = "/idx/_plugins/_asynchronous_search";

fn submit(s: &Service, body: &str, query: &[(&str, &str)]) -> Result<Value, RestError> {
    let query = q(query);
    let w = who();
    let r = s.submit(&Submit {
        index: Some("idx"),
        path: PATH,
        query: &query,
        body: body.as_bytes(),
        who: &w,
    })?;
    assert_eq!(r.status, 200);
    Ok(serde_json::from_slice(&r.body).expect("json"))
}

fn get(s: &Service, id: &str, query: &[(&str, &str)]) -> Result<Value, RestError> {
    let r = s.get(&who(), id, "/_plugins/_asynchronous_search/x", &q(query))?;
    Ok(serde_json::from_slice(&r.body).expect("json"))
}

fn reason(e: &RestError) -> (u16, String, String) {
    let mut out = Vec::new();
    e.write_body(&mut out).expect("body");
    let v: Value = serde_json::from_slice(&out).expect("json");
    (
        e.status(),
        v["error"]["type"].as_str().unwrap_or_default().to_owned(),
        v["error"]["reason"].as_str().unwrap_or_default().to_owned(),
    )
}

// AC-1, AC-4: finished inside the wait and not kept is CLOSED with an id, and
// the id is gone after.
#[test]
fn a_finished_search_that_is_not_kept_is_closed_and_then_gone() {
    let (s, _) = rig();
    let v = submit(&s, "{}", &[]).expect("answer");
    assert_eq!(v["state"], "CLOSED");
    assert_eq!(v["response"]["took"], 1);
    assert!(v["start_time_in_millis"].is_u64() && v["expiration_time_in_millis"].is_u64());
    let id = v["id"].as_str().expect("id").to_owned();
    let (status, kind, why) = reason(&get(&s, &id, &[]).expect_err("gone"));
    assert_eq!(
        (status, kind.as_str()),
        (404, "resource_not_found_exception")
    );
    assert_eq!(
        why,
        format!("Either the resource [{id}] does not exist or you do not have access")
    );
    assert_eq!(reason(&s.delete(&who(), &id).expect_err("gone")).0, 404);
}

// AC-4: kept is PERSISTING at the submit, STORE_RESIDENT later, deletable once.
#[test]
fn a_kept_search_is_persisting_then_store_resident_until_deleted() {
    let (s, _) = rig();
    let v = submit(&s, "{}", &[("keep_on_completion", "true")]).expect("answer");
    assert_eq!(v["state"], "PERSISTING");
    let id = v["id"].as_str().expect("id").to_owned();
    for query in [
        vec![],
        vec![("keep_alive", "2d")],
        vec![("wait_for_completion_timeout", "1s")],
    ] {
        let r = get(&s, &id, &query);
        if query.iter().any(|(k, _)| *k == "keep_alive") {
            assert_eq!(
                reason(&r.expect_err("over the limit")).1,
                "illegal_argument_exception"
            );
        } else {
            assert_eq!(r.expect("kept")["state"], "STORE_RESIDENT");
        }
    }
    assert_eq!(
        s.delete(&who(), &id).expect("deleted").body,
        br#"{"acknowledged":true}"#
    );
    assert_eq!(reason(&get(&s, &id, &[]).expect_err("gone")).0, 404);
    assert_eq!(reason(&s.delete(&who(), &id).expect_err("gone")).0, 404);
}

// AC-2, AC-4, AC-6: running is RUNNING with the partial response; a kept one
// becomes STORE_RESIDENT, one that is not kept is gone; delete cancels.
#[test]
fn a_running_search_shows_its_partial_response_and_follows_the_keep_rule() {
    let (s, gate) = rig();
    let wait = ("wait_for_completion_timeout", "1ms");
    let kept = submit(&s, "slow", &[wait, ("keep_on_completion", "true")]).expect("answer");
    let loose = submit(&s, "slow", &[wait]).expect("answer");
    for v in [&kept, &loose] {
        assert_eq!(v["state"], "RUNNING");
        assert_eq!(v["response"]["_shards"]["successful"], 0);
        assert_eq!(
            get(&s, v["id"].as_str().expect("id"), &[]).expect("running")["state"],
            "RUNNING"
        );
    }
    let third = submit(&s, "slow", &[wait]).expect("answer");
    let id3 = third["id"].as_str().expect("id").to_owned();
    assert_eq!(s.delete(&who(), &id3).expect("cancelled").status, 200);
    assert_eq!(reason(&get(&s, &id3, &[]).expect_err("cancelled")).0, 404);
    gate.open();
    let id = kept["id"].as_str().expect("id");
    let done = get(&s, id, &[("wait_for_completion_timeout", "5s")]).expect("finished");
    assert_eq!(done["state"], "STORE_RESIDENT");
    assert_eq!(done["response"]["took"], 1);
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(
        reason(&get(&s, loose["id"].as_str().expect("id"), &[]).expect_err("dropped")).0,
        404
    );
}

// AC-3: an error while it runs is a 200 with the error; a refusal at plan is a
// synchronous 400 with no job.
#[test]
fn errors_are_answered_where_the_recording_answers_them() {
    let (s, _) = rig();
    let v = submit(&s, "error", &[]).expect("answer");
    assert_eq!(
        (v["state"].as_str(), v["error"]["type"].as_str()),
        (Some("CLOSED"), Some("index_not_found_exception"))
    );
    assert!(v.get("response").is_none());
    let (status, kind, _) =
        reason(&submit(&s, "fail", &[("keep_on_completion", "true")]).expect_err("refused"));
    assert_eq!((status, kind.as_str()), (400, "parsing_exception"));
    assert!(!s.stats().body.is_empty());
    let stats: Value = serde_json::from_slice(&s.stats().body).expect("json");
    assert_eq!(
        stats["nodes"]["n1"]["asynchronous_search_stats"]["submitted"],
        1
    );
}

// AC-3, AC-5: parameters, with the recording's messages.
#[test]
fn parameters_are_refused_with_the_recorded_messages() {
    let (s, _) = rig();
    let cases = [
        (
            ("keep_alive", "1ms"),
            400,
            "action_request_validation_exception",
            "Validation Failed: 1: [keep_alive] must be greater than 1 minute, got: 1ms;",
        ),
        (
            ("keep_alive", "soon"),
            400,
            "illegal_argument_exception",
            "failed to parse setting [keep_alive] with value [soon] as a time value: unit is missing or unrecognized",
        ),
        (
            ("wait_for_completion_timeout", "soon"),
            400,
            "illegal_argument_exception",
            "failed to parse setting [wait_for_completion_timeout] with value [soon] as a time value: unit is missing or unrecognized",
        ),
        (
            ("keep_on_completion", "maybe"),
            400,
            "illegal_argument_exception",
            "Failed to parse value [maybe] as only [true] or [false] are allowed.",
        ),
        (
            ("nope", "1"),
            400,
            "illegal_argument_exception",
            "request [/idx/_plugins/_asynchronous_search] contains unrecognized parameter: [nope]",
        ),
    ];
    for ((name, value), status, kind, why) in cases {
        let e = submit(&s, "{}", &[(name, value)]).expect_err(name);
        assert_eq!(
            reason(&e),
            (status, kind.to_owned(), why.to_owned()),
            "{name}"
        );
    }
    let e = submit(&s, "{}", &[("keep_alive", "2h")]).expect_err("over the limit");
    assert!(
        reason(&e)
            .2
            .starts_with("Keep alive for asynchronous search ("),
        "{}",
        reason(&e).2
    );
    let e = submit(&s, "{}", &[("wait_for_completion_timeout", "11s")]).expect_err("too long");
    assert_eq!(reason(&e).0, 400);
    assert!(!s.stats().body.is_empty());
    assert!(
        submit(
            &s,
            "{}",
            &[
                ("routing", "r"),
                ("preference", "_local"),
                ("request_cache", "true")
            ]
        )
        .is_ok()
    );
}

// AC-3, AC-12: ids that are not this caller's jobs on this node are 404.
#[test]
fn an_id_that_is_not_a_job_of_this_caller_is_not_found() {
    let (s, _) = rig();
    let v = submit(&s, "{}", &[("keep_on_completion", "true")]).expect("answer");
    let id = v["id"].as_str().expect("id").to_owned();
    for bad in ["not-an-id".to_owned(), "n2:7".to_owned(), String::new()] {
        assert_eq!(
            reason(&get(&s, &bad, &[]).expect_err("not found")).0,
            404,
            "{bad}"
        );
    }
    let other = Who {
        tenant: "x".into(),
        user: "u".into(),
    };
    assert_eq!(
        reason(&s.get(&other, &id, "/p", &[]).expect_err("not yours")).0,
        404
    );
    assert_eq!(get(&s, &id, &[]).expect("mine")["state"], "STORE_RESIDENT");
    let _ = json!(null);
}

/// A backend whose search runs until it is told to stop, as a search over many
/// shards does between them.
struct Stoppable;

impl crate::sql_route::SqlBackend for Stoppable {
    fn mapping(&self, _: &str) -> Result<Value, crate::sql_route::SqlFailure> {
        Ok(json!({}))
    }

    fn search(&self, _: &[String], _: &Value) -> Result<Vec<u8>, crate::sql_route::SqlFailure> {
        panic!("a search by the runner is told how to stop");
    }

    fn search_until(
        &self,
        _: &[String],
        _: &Value,
        stop: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<u8>, crate::sql_route::SqlFailure> {
        while !stop() {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        Err(crate::sql_route::SqlFailure::Body(
            400,
            br#"{"error":{"type":"task_cancelled_exception","reason":"cancelled"}}"#.to_vec(),
        ))
    }
}

// AC-6: a delete reaches the search itself, which stops, and the slot is freed
// when its thread has.
#[test]
fn a_delete_stops_the_running_search_and_frees_its_slot() {
    let config = BudgetConfig::new(256 << 20)
        .with_pool(Pool::Extensions, 128 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let clock: Clock = Arc::new(|| 1_000);
    let jobs = Jobs::new("n1", &budget, Limits::default(), clock);
    let s = Service::new(
        Arc::clone(&jobs),
        Arc::new(SearchRunner::new(Arc::new(Stoppable))),
        "opensearch",
        "n1",
    );
    let v = submit(&s, "{}", &[("wait_for_completion_timeout", "1ms")]).expect("answer");
    assert_eq!(v["state"], "RUNNING");
    assert_eq!(jobs.stats().running_current, 1);
    let id = v["id"].as_str().expect("id").to_owned();
    assert_eq!(s.delete(&who(), &id).expect("cancelled").status, 200);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while jobs.stats().running_current != 0 {
        assert!(
            std::time::Instant::now() < until,
            "the search never stopped"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

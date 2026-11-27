//! docs/specs/node-tasks: `wait_for_completion=false`, `GET /_tasks` and
//! cancel over the by-query rig, the walk held at a gate.

// ARITH: offsets into a reply are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

use std::time::{Duration, Instant};

use super::by_query_rig::{QUERY, Rig, call, put, requests, rig, source_of};
use crate::support;
use oss_coordinator::docs::by_query::Hit;

fn task_id(reply: &support::Reply) -> String {
    let text = reply.text();
    assert_eq!(reply.status, 200, "{text}");
    let at = text.find(r#""task":""#).expect("task") + 8;
    text[at..].split('"').next().expect("id").to_owned()
}

fn finished(r: &Rig, id: &str) -> String {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let text = call(r, "GET", &format!("/_tasks/{id}"), None).text();
        if text.contains(r#""completed":true"#) {
            return text;
        }
        assert!(Instant::now() < until, "still running: {text}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn reindex(extra: &str) -> String {
    format!(
        r#"{{"source":{{"index":"docs","query":{{"match":{{"n":1}}}}}},"dest":{{"index":"dest"}}{extra}}}"#
    )
}

fn hold_hits(r: &Rig) {
    let hits: Vec<Hit> = ["a", "b"].iter().map(|id| put(r, id)).collect();
    *r.matches.hits.lock().unwrap() = hits;
}

// docs/specs/node-tasks AC-1, AC-2, AC-7
#[test]
fn a_background_request_answers_a_task_that_finishes_with_the_synchronous_counts() {
    let r = rig();
    hold_hits(&r);
    let before = requests(&r);
    let sync = call(&r, "POST", "/docs/_delete_by_query", Some(QUERY));
    assert_eq!(sync.status, 200, "{}", sync.text());
    let sync_requests = requests(&r) - before;
    // The same request again as a task, held at the gate.
    hold_hits(&r);
    r.matches.close();
    let before = requests(&r);
    let reply = call(
        &r,
        "POST",
        "/docs/_delete_by_query?wait_for_completion=false",
        Some(QUERY),
    );
    let id = task_id(&reply);
    assert!(id.starts_with("node-1:"), "{id}");
    let running = call(&r, "GET", &format!("/_tasks/{id}"), None).text();
    for part in [
        r#""completed":false"#,
        r#""action":"indices:data/write/delete/byquery""#,
        r#""description":"delete-by-query [docs]""#,
        r#""cancellable":true"#,
    ] {
        assert!(running.contains(part), "{part} in {running}");
    }
    assert!(!running.contains(r#""response""#), "{running}");
    let listing = call(&r, "GET", "/_tasks", None).text();
    assert!(listing.contains(&id), "{listing}");
    let none = call(
        &r,
        "GET",
        "/_tasks?actions=indices:data/write/reindex",
        None,
    )
    .text();
    assert!(!none.contains(&id), "{none}");
    r.matches.open();
    let done = finished(&r, &id);
    assert!(done.contains(r#""response":{"#), "{done}");
    assert!(done.contains(r#""total":2,"deleted":2"#), "{done}");
    assert!(
        !call(&r, "GET", "/_tasks", None).text().contains(&id),
        "a finished task is not listed"
    );
    assert_eq!(
        requests(&r) - before,
        sync_requests,
        "no blob request for the task"
    );
}

// docs/specs/node-tasks AC-1
#[test]
fn update_by_query_and_reindex_run_as_tasks_too() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let r = rig();
    hold_hits(&r);
    let update = call(
        &r,
        "POST",
        "/docs/_update_by_query?wait_for_completion=false",
        Some(r#"{"query":{"match":{"n":1}},"script":{"source":"ctx._source.n += 4"}}"#),
    );
    let id = task_id(&update);
    let done = finished(&r, &id);
    assert!(done.contains(r#""updated":2"#), "{done}");
    assert!(done.contains("update/byquery"), "{done}");
    assert!(source_of(&r, "a").contains(r#""n":5"#));
    let copy = call(
        &r,
        "POST",
        "/_reindex?wait_for_completion=false",
        Some(&reindex("")),
    );
    let done = finished(&r, &task_id(&copy));
    assert!(done.contains(r#""created":2"#), "{done}");
    assert!(done.contains("reindex from [docs] to [dest]"), "{done}");
    oss_reader::painless_switch::set_native_compile(false);
}

// docs/specs/node-tasks AC-3
#[test]
fn cancel_stops_a_running_task_and_a_finished_one_is_not_found() {
    let r = rig();
    hold_hits(&r);
    r.matches.close();
    let id = task_id(&call(
        &r,
        "POST",
        "/_reindex?wait_for_completion=false",
        Some(&reindex("")),
    ));
    let cancel = call(&r, "POST", &format!("/_tasks/{id}/_cancel"), None);
    assert_eq!(cancel.status, 200, "{}", cancel.text());
    assert!(cancel.text().contains(&id), "{}", cancel.text());
    r.matches.open();
    let done = finished(&r, &id);
    assert!(done.contains(r#""canceled":"by user request""#), "{done}");
    assert!(done.contains(r#""created":0"#), "{done}");
    assert!(done.contains(r#""cancelled":true"#), "{done}");
    let again = call(&r, "POST", &format!("/_tasks/{id}/_cancel"), None);
    assert_eq!(again.status, 404, "{}", again.text());
}

// docs/specs/node-tasks AC-4
#[test]
fn a_failing_task_has_an_error_and_a_refusal_stays_synchronous() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let r = rig();
    hold_hits(&r);
    let id = task_id(&call(
        &r,
        "POST",
        "/_reindex?wait_for_completion=false",
        Some(&reindex(
            r#","script":{"source":"ctx._source.n = ctx.nope.x"}"#,
        )),
    ));
    let done = finished(&r, &id);
    assert!(
        done.contains(r#""error":{"#) && done.contains("script_exception"),
        "{done}"
    );
    let missing = call(
        &r,
        "POST",
        "/nope/_delete_by_query?wait_for_completion=false",
        Some(QUERY),
    );
    assert_eq!(missing.status, 404, "{}", missing.text());
    let bad = call(
        &r,
        "POST",
        "/docs/_delete_by_query?wait_for_completion=false&conflicts=bad",
        Some(QUERY),
    );
    assert_eq!(bad.status, 400, "{}", bad.text());
    oss_reader::painless_switch::set_native_compile(false);
}

// docs/specs/node-tasks AC-5
#[test]
fn the_thirty_third_running_task_is_refused() {
    let r = rig();
    hold_hits(&r);
    r.matches.close();
    let ids: Vec<String> = (0..32)
        .map(|_| {
            task_id(&call(
                &r,
                "POST",
                "/docs/_delete_by_query?wait_for_completion=false",
                Some(QUERY),
            ))
        })
        .collect();
    let over = call(
        &r,
        "POST",
        "/docs/_delete_by_query?wait_for_completion=false",
        Some(QUERY),
    );
    assert_eq!(over.status, 429, "{}", over.text());
    assert!(over.text().contains("es_rejected_execution_exception"));
    r.matches.open();
    for id in ids {
        finished(&r, &id);
    }
}

// docs/specs/node-tasks AC-7
#[test]
fn another_nodes_or_a_malformed_id_is_refused() {
    let r = rig();
    let other = call(&r, "GET", "/_tasks/node-9:5", None);
    assert_eq!(other.status, 404, "{}", other.text());
    assert!(other.text().contains("node-9"), "{}", other.text());
    let own = call(&r, "GET", "/_tasks/node-1:5", None);
    assert_eq!(own.status, 404, "{}", own.text());
    let bad = call(&r, "GET", "/_tasks/junk", None);
    assert_eq!(bad.status, 400, "{}", bad.text());
    let param = call(&r, "GET", "/_tasks?detailed=true", None);
    assert_eq!(param.status, 400, "{}", param.text());
}

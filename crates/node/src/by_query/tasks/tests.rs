//! The registry's bounds, with the clock passed in.

use super::*;
use oss_mem::BudgetConfig;

fn budget(request_bodies: usize) -> MemoryBudget {
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::RequestBodies, request_bodies)
        .expect("fits");
    MemoryBudget::new(&config)
}

fn tasks() -> Tasks {
    Tasks::new("n1", &budget(32 << 20), 1_000)
}

fn status(e: &RestError) -> u16 {
    e.status()
}

// docs/specs/node-tasks AC-5
#[test]
fn the_running_and_kept_bounds_hold() {
    let t = tasks();
    let ids: Vec<u64> = (0..MAX_RUNNING)
        .map(|_| t.start("a", "d", 0).expect("room").0)
        .collect();
    assert_eq!(status(&t.start("a", "d", 0).expect_err("full")), 429);
    for id in &ids {
        t.finish(*id, Ok("{}".into()), 1);
    }
    for _ in 0..(MAX_KEPT + 8) {
        let (id, _) = t.start("a", "d", 2).expect("room");
        t.finish(id, Ok("{}".into()), 2);
    }
    assert_eq!(t.get(ids[0], 3).expect_err("dropped").status(), 404);
    assert!(t.lock().kept.len() <= MAX_KEPT);
}

// docs/specs/node-tasks AC-5
#[test]
fn a_finished_task_expires_and_the_budget_bounds_a_start() {
    let t = tasks();
    let (id, _) = t.start("a", "d", 0).expect("room");
    t.finish(id, Ok("{}".into()), 10);
    assert!(t.get(id, 10 + TTL_MILLIS - 1).is_ok());
    assert_eq!(
        t.get(id, 10 + TTL_MILLIS).expect_err("expired").status(),
        404
    );
    let small = Tasks::new("n1", &budget(ENTRY_BYTES), 0);
    small.start("a", "d", 0).expect("one fits");
    assert_eq!(small.start("a", "d", 0).expect_err("no more").status(), 429);
}

// docs/specs/node-tasks AC-5
#[test]
fn an_entry_over_its_bound_drops_failures_and_truncates_text() {
    let big = format!(
        r#"{{"took":1,"total":1,"failures":["{}"]}}"#,
        "x".repeat(RESPONSE_MAX * 2)
    );
    assert_eq!(fit_response(big), r#"{"took":1,"total":1,"failures":[]}"#);
    let t = tasks();
    let (id, _) = t.start("a", &"é".repeat(TEXT_MAX), 0).expect("room");
    let body = t.get(id, 0).expect("kept");
    assert!(body.len() < TEXT_MAX * 2 + 400, "{}", body.len());
}

// docs/specs/node-tasks AC-6
#[test]
fn a_thread_that_ends_without_a_result_frees_its_slot() {
    let t = Arc::new(tasks());
    let (id, _) = t.start("a", "d", 0).expect("room");
    let slot = Slot::new(Arc::clone(&t), id, || 5);
    let ended = std::thread::spawn(move || {
        let _slot = slot;
        panic!("the walk panicked");
    })
    .join();
    assert!(ended.is_err());
    assert!(t.lock().running.is_empty());
    let body = t.get(id, 5).expect("kept");
    assert!(
        body.contains(r#""completed":true"#) && body.contains(r#""error":"#),
        "{body}"
    );
}

// docs/specs/node-tasks AC-6, AC-7
#[test]
fn ids_start_at_the_start_time_and_other_nodes_are_refused() {
    let t = tasks();
    let (id, _) = t.start("a", "d", 0).expect("room");
    assert_eq!(id, 1_000);
    assert_eq!(t.parse("n1:1000").expect("own"), 1_000);
    assert_eq!(t.parse("n2:1000").expect_err("other node").status(), 404);
    assert_eq!(t.parse("junk").expect_err("malformed").status(), 400);
    assert_eq!(t.parse("n1:x").expect_err("malformed").status(), 400);
}

// docs/specs/node-tasks AC-2
#[test]
fn the_listing_has_running_tasks_only_and_filters_by_action() {
    let t = tasks();
    let (a, _) = t.start("indices:data/write/reindex", "r", 0).expect("room");
    let (b, _) = t
        .start("indices:data/write/delete/byquery", "d", 0)
        .expect("room");
    t.finish(b, Ok("{}".into()), 1);
    let all = t.running_json(None, 2);
    assert!(
        all.contains(&format!("n1:{a}")) && !all.contains(&format!("n1:{b}")),
        "{all}"
    );
    assert!(
        t.running_json(Some("indices:data/write/delete/*"), 2)
            .contains(r#""tasks":{}"#)
    );
    assert!(
        t.running_json(Some("indices:data/write/reindex"), 2)
            .contains(&format!("n1:{a}"))
    );
}

// docs/specs/node-tasks AC-8
#[test]
fn the_compatibility_notes_record_each_difference() {
    let notes = include_str!("../../../../../docs/compatibility.md");
    for part in [
        "no blob request and no `.tasks` index",
        "lost at a restart",
        "an id of another node is 404",
        "running tasks only",
        "shows no `status`",
    ] {
        assert!(notes.contains(part), "{part}");
    }
}

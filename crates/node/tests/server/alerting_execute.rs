//! docs/specs/alerting AC-3: one run of a query-level monitor through
//! `_execute`.

use crate::alerting_routes::{BASE, call, monitor, named, rig};
use crate::support;

use oss_blob::{BlobStore, S3Op};
use serde_json::{Value, json};

fn with_condition(source: &str) -> Value {
    let mut m = monitor("run", false);
    m["triggers"][0]["query_level_trigger"]["condition"]["script"]["source"] = json!(source);
    m
}

#[test]
fn an_execute_evaluates_each_trigger_and_stores_nothing() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let r = rig();
    let id = named(&r, "run", false);
    let writes = || r.store.meter().count(S3Op::Put) + r.store.meter().count(S3Op::Delete);
    let before = writes();
    let run = |target: String, body: Option<&Value>| call(&r, "POST", &target, body);
    let (status, stored) = run(format!("{BASE}/{id}/_execute?dryrun=true"), None);
    assert_eq!(status, 200, "{stored}");
    let trig = stored["trigger_results"].as_object().expect("triggers");
    assert_eq!(trig.len(), 1);
    let one = trig.values().next().expect("one");
    assert_eq!(
        (one["triggered"].as_bool(), one["error"].is_null()),
        (Some(true), true)
    );
    assert_eq!(
        stored["input_results"]["results"][0]["hits"]["total"]["value"],
        4
    );
    for (source, want) in [
        ("ctx.results[0].hits.total.value > 100", false),
        ("ctx.results[0].hits.total.value == 4;", true),
        ("false", false),
    ] {
        let (status, got) = run(
            format!("{BASE}/_execute?dryrun=true"),
            Some(&with_condition(source)),
        );
        assert_eq!(status, 200, "{got}");
        let t = got["trigger_results"]
            .as_object()
            .expect("triggers")
            .values()
            .next()
            .expect("one");
        assert_eq!(
            (t["triggered"].as_bool(), t["error"].is_null()),
            (Some(want), true),
            "{source}: {got}"
        );
    }
    let (_, bad) = run(
        format!("{BASE}/_execute"),
        Some(&with_condition("ctx.nope(")),
    );
    let t = bad["trigger_results"]
        .as_object()
        .expect("triggers")
        .values()
        .next()
        .expect("one");
    assert!(t["error"].is_string(), "{bad}");
    assert_eq!(writes(), before, "an execute writes nothing");
    let (status, missing) = run(format!("{BASE}/nope/_execute"), None);
    assert_eq!(
        (status, missing["error"]["reason"].as_str()),
        (404, Some("Can't find monitor with id: nope"))
    );
    let mut bucket = monitor("b", false);
    bucket["monitor_type"] = json!("bucket_level_monitor");
    assert_eq!(run(format!("{BASE}/_execute"), Some(&bucket)).0, 501);
    oss_reader::painless_switch::set_native_compile(false);
    let (_, off) = run(format!("{BASE}/{id}/_execute"), None);
    let t = off["trigger_results"]
        .as_object()
        .expect("triggers")
        .values()
        .next()
        .expect("one");
    assert!(
        t["error"]
            .as_str()
            .is_some_and(|e| e.contains("native_compile")),
        "{off}"
    );
}

fn alerts_of(r: &crate::alerting_routes::Rig, id: &str) -> Vec<Value> {
    let (status, got) = call(r, "GET", &format!("{BASE}/alerts?monitorId={id}"), None);
    assert_eq!(status, 200, "{got}");
    got["alerts"].as_array().cloned().unwrap_or_default()
}

#[test]
fn a_run_opens_keeps_and_completes_an_alert_and_an_acknowledge_moves_it() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let r = rig();
    let id = named(&r, "run", false);
    let execute = |dry: bool| {
        call(
            &r,
            "POST",
            &format!("{BASE}/{id}/_execute?dryrun={dry}"),
            None,
        )
        .0
    };
    assert_eq!(execute(true), 200);
    assert!(alerts_of(&r, &id).is_empty(), "a dry run opens none");
    assert_eq!(execute(false), 200);
    let open = alerts_of(&r, &id);
    assert_eq!(open.len(), 1);
    assert_eq!(
        (open[0]["state"].as_str(), open[0]["version"].as_i64()),
        (Some("ACTIVE"), Some(1))
    );
    let writes = || r.store.meter().count(S3Op::Put);
    let before = writes();
    assert_eq!(execute(false), 200);
    assert_eq!(
        writes(),
        before,
        "a run that changes nothing writes nothing"
    );
    let alert = open[0]["id"].as_str().expect("an id").to_owned();
    let ack = |ids: Value| {
        call(
            &r,
            "POST",
            &format!("{BASE}/{id}/_acknowledge/alerts"),
            Some(&json!({"alerts": ids})),
        )
    };
    let (status, done) = ack(json!([alert, "gone"]));
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["success"], json!([alert]));
    assert_eq!(done["failed"].as_array().map(Vec::len), Some(1));
    assert_eq!(alerts_of(&r, &id)[0]["state"], "ACKNOWLEDGED");
    assert_eq!(execute(false), 200);
    assert_eq!(
        alerts_of(&r, &id)[0]["state"],
        "ACKNOWLEDGED",
        "a run keeps an acknowledged alert"
    );
    // The condition stops holding: the alert completes and leaves the open set.
    let (status, _) = call(
        &r,
        "PUT",
        &format!("{BASE}/{id}"),
        Some(&with_condition("false")),
    );
    assert_eq!(status, 200);
    assert_eq!(execute(false), 200);
    assert!(alerts_of(&r, &id).is_empty());
    // A condition that fails is an ERROR alert, and one that holds again clears the error.
    let (status, _) = call(
        &r,
        "PUT",
        &format!("{BASE}/{id}"),
        Some(&with_condition("ctx.nope.nope > 1")),
    );
    assert_eq!(status, 200);
    assert_eq!(execute(false), 200);
    let failing = alerts_of(&r, &id);
    assert_eq!(failing[0]["state"], "ERROR");
    assert!(failing[0]["error_message"].is_string());
    let (_, sev) = call(
        &r,
        "GET",
        &format!("{BASE}/alerts?severityLevel=2&monitorId={id}"),
        None,
    );
    assert_eq!(sev["totalAlerts"], 0);
    let (_, all) = call(&r, "GET", &format!("{BASE}/alerts?alertState=ERROR"), None);
    assert_eq!(all["totalAlerts"], 1);
    let (status, missing) = call(
        &r,
        "POST",
        &format!("{BASE}/nope/_acknowledge/alerts"),
        Some(&json!({"alerts": []})),
    );
    assert_eq!(
        (status, missing["error"]["reason"].as_str()),
        (500, Some("No monitor found with id [nope]"))
    );
    oss_reader::painless_switch::set_native_compile(false);
}

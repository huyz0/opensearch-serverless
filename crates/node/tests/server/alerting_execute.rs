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

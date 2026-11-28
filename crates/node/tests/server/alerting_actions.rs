//! docs/specs/alerting AC-9: a completed alert is written to the history
//! index, and the actions of an ACTIVE alert are sent, throttled and
//! recorded.

use crate::alerting_routes::{BASE, call, monitor, rig};
use crate::support;

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use oss_blob::BlobStore;
use oss_ext::doc_sink::{Doc, DocResult, DocSink};
use oss_ext::host::HostError;
use oss_node::ad_route::WindowSearch;
use oss_node::alerting_route::{ActionSender, AlertingService};
use oss_rest::RestError;
use serde_json::{Value, json};

struct Hits(Arc<AtomicI64>);

impl WindowSearch for Hits {
    fn search(&self, _: &[String], _: &Value) -> Result<Value, RestError> {
        Ok(json!({
            "took": 1, "timed_out": false,
            "_shards": {"total": 1, "successful": 1, "skipped": 0, "failed": 0},
            "hits": {"total": {"value": self.0.load(Ordering::SeqCst), "relation": "eq"},
                "max_score": null, "hits": []},
        }))
    }
}

#[derive(Default)]
struct Sent {
    messages: Mutex<Vec<(String, String)>>,
    fail: Mutex<bool>,
}

impl ActionSender for Sent {
    fn send(&self, destination: &str, text: &str) -> Result<(), String> {
        self.messages
            .lock()
            .unwrap()
            .push((destination.to_owned(), text.to_owned()));
        if *self.fail.lock().unwrap() {
            Err("not found".to_owned())
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
struct History(Mutex<Vec<(String, Doc)>>);

impl DocSink for History {
    fn index(&self, index: &str, docs: &[Doc]) -> Result<Vec<DocResult>, HostError> {
        let mut held = self.0.lock().unwrap();
        for d in docs {
            held.push((index.to_owned(), d.clone()));
        }
        Ok(docs
            .iter()
            .map(|d| DocResult {
                id: d.id.clone().unwrap_or_default(),
                status: 201,
            })
            .collect())
    }
}

struct World {
    r: crate::alerting_routes::Rig,
    service: AlertingService,
    hits: Arc<AtomicI64>,
    sent: Arc<Sent>,
    history: Arc<History>,
    id: String,
    stored: Value,
    version: i64,
}

fn world(throttle: bool) -> World {
    let r = rig();
    let blobs = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let hits = Arc::new(AtomicI64::new(4));
    let sent = Arc::new(Sent::default());
    let history = Arc::new(History::default());
    let service = AlertingService::new(&blobs, "")
        .with_search(Arc::new(Hits(Arc::clone(&hits))))
        .with_sender(Arc::clone(&sent) as _)
        .with_history(Arc::clone(&history) as _);
    let mut body = monitor("acts", true);
    body["triggers"][0]["query_level_trigger"]["actions"] = json!([{
        "name": "tell", "destination_id": "cfg-1",
        "message_template": {"source": "{{ctx.monitor.name}} found {{ctx.results.0.hits.total.value}} ({{ctx.trigger.name}})"},
        "throttle_enabled": throttle,
        "throttle": {"value": 60, "unit": "MINUTES"},
    }]);
    let (status, made) = call(&r, "POST", BASE, Some(&body));
    assert_eq!(status, 201, "{made}");
    World {
        service,
        hits,
        sent,
        history,
        id: made["_id"].as_str().expect("id").to_owned(),
        stored: made["monitor"].clone(),
        version: made["_version"].as_i64().unwrap_or(1),
        r,
    }
}

impl World {
    fn run(&self, at: i64) {
        self.service
            .run_scheduled("_default_", &self.id, self.version, &self.stored, at)
            .expect("a run");
    }

    fn alert(&self) -> Value {
        let (_, got) = call(
            &self.r,
            "GET",
            &format!("{BASE}/alerts?monitorId={}", self.id),
            None,
        );
        got["alerts"][0].clone()
    }
}

#[test]
fn an_active_alert_sends_its_rendered_action_and_a_throttle_holds_the_next() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let w = world(true);
    w.run(1_000_000);
    w.run(1_000_000 + 60_000);
    let sent = w.sent.messages.lock().unwrap().clone();
    assert_eq!(
        sent,
        vec![("cfg-1".to_owned(), "acts found 4 (t)".to_owned())],
        "the second run is inside the throttle"
    );
    let alert = w.alert();
    assert_eq!(alert["state"], "ACTIVE", "{alert}");
    assert!(alert["action_execution_results"][0]["action_id"].is_string());
    // Past the window the action is due again.
    w.run(1_000_000 + 61 * 60_000);
    assert_eq!(w.sent.messages.lock().unwrap().len(), 2);
    oss_reader::painless_switch::set_native_compile(false);
}

#[test]
fn an_unthrottled_action_is_sent_on_every_run() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let w = world(false);
    w.run(1_000_000);
    w.run(1_300_000);
    assert_eq!(w.sent.messages.lock().unwrap().len(), 2);
    oss_reader::painless_switch::set_native_compile(false);
}

#[test]
fn a_failed_send_leaves_the_alert_in_error_and_it_is_sent_again() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let w = world(true);
    *w.sent.fail.lock().unwrap() = true;
    w.run(1_000_000);
    let alert = w.alert();
    assert_eq!(alert["state"], "ERROR", "{alert}");
    assert!(
        alert["error_message"]
            .as_str()
            .is_some_and(|m| m.contains("tell") && m.contains("not found")),
        "{alert}"
    );
    *w.sent.fail.lock().unwrap() = false;
    w.run(1_000_000 + 60_000);
    assert_eq!(w.sent.messages.lock().unwrap().len(), 2, "tried again");
    assert_eq!(w.alert()["state"], "ACTIVE");
    oss_reader::painless_switch::set_native_compile(false);
}

#[test]
fn a_cleared_alert_is_written_to_the_history_index_as_completed() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let w = world(false);
    w.run(1_000_000);
    let open = w.alert();
    assert!(
        w.history.0.lock().unwrap().is_empty(),
        "nothing completed yet"
    );
    w.hits.store(0, Ordering::SeqCst);
    w.run(1_300_000);
    let (_, alerts) = call(
        &w.r,
        "GET",
        &format!("{BASE}/alerts?monitorId={}", w.id),
        None,
    );
    assert_eq!(alerts["totalAlerts"], 0, "{alerts}");
    let held = w.history.0.lock().unwrap().clone();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].0, ".opendistro-alerting-alert-history-write");
    assert_eq!(held[0].1.id.as_deref(), open["id"].as_str());
    let doc: Value = serde_json::from_slice(&held[0].1.source).expect("json");
    assert_eq!(doc["state"], "COMPLETED");
    assert!(doc["end_time"].is_i64());
    assert_eq!(doc["monitor_id"], w.id.as_str());
    oss_reader::painless_switch::set_native_compile(false);
}

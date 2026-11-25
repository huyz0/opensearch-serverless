//! docs/specs/rollover-route AC-8 and AC-9: the conditions and the body of an
//! alias rollover that shape the index it moves to.

use crate::rollover_route::{BUF, Counted, call, indices_of, json, rig, rig_full};

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oss_blob::BlobStore;
use oss_meta::{DeleteMode, DescriptorStore};

// AC-8
#[test]
fn max_docs_is_decided_by_the_count_of_the_index_left() {
    let counter = Arc::new(Counted {
        docs: AtomicU64::new(3),
        asked: std::sync::Mutex::new(Vec::new()),
    });
    let r = rig_full(None, Some(Arc::clone(&counter)));
    assert_eq!(call(&r, "PUT", "/_data_stream/logs-1", None).status, 200);
    let body = r#"{"conditions":{"max_docs":5}}"#;
    let few = json(&call(&r, "POST", "/logs-1/_rollover", Some(body)));
    assert_eq!(few["rolled_over"], false);
    assert_eq!(few["conditions"]["[max_docs: 5]"], false);
    assert_eq!(indices_of(&r, "logs-1").len(), 1);
    counter.docs.store(5, Ordering::Relaxed);
    let many = json(&call(&r, "POST", "/logs-1/_rollover", Some(body)));
    assert_eq!(many["rolled_over"], true);
    assert_eq!(many["conditions"]["[max_docs: 5]"], true);
    assert_eq!(indices_of(&r, "logs-1").len(), 2);
    // An unconditional rollover never counts.
    assert_eq!(call(&r, "POST", "/logs-1/_rollover", None).status, 200);
    let asked = counter.asked.lock().expect("lock").clone();
    assert_eq!(
        asked,
        vec![
            (".ds-logs-1-000001".to_owned(), 1),
            (".ds-logs-1-000001".to_owned(), 1)
        ]
    );
    // Size conditions stay refused.
    let size = call(
        &r,
        "POST",
        "/logs-1/_rollover",
        Some(r#"{"conditions":{"max_size":"1gb"}}"#),
    );
    assert_eq!(size.status, 501);
}

// AC-9
#[test]
fn an_alias_rollover_creates_the_index_with_the_settings_mappings_and_aliases_of_the_body() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/app-000001", None).status, 200);
    assert_eq!(
        call(
            &r,
            "PUT",
            "/app-000001/_alias/app",
            Some(r#"{"is_write_index":true}"#)
        )
        .status,
        200
    );
    let body = r#"{"settings":{"index":{"number_of_shards":2}},"mappings":{"properties":{"n":{"type":"integer"}}},"aliases":{"reads":{},"latest":{"is_write_index":true}}}"#;
    let rolled = call(&r, "POST", "/app/_rollover", Some(body));
    assert_eq!(rolled.status, 200, "{}", rolled.text());
    assert_eq!(json(&rolled)["rolled_over"], true);
    let mut buf = vec![0u8; BUF];
    let store = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let descriptors = DescriptorStore::new(store, "", DeleteMode::Conditional).expect("names");
    let record = descriptors
        .get("app-000002", &mut buf)
        .expect("reads")
        .expect("the new index");
    assert_eq!(record.number_of_shards, 2);
    let mapping: serde_json::Value =
        serde_json::from_str(record.mapping.as_deref().expect("a mapping")).expect("json");
    assert_eq!(mapping["properties"]["n"]["type"], "integer");
    let held = |alias: &str| -> Vec<String> {
        let got = json(&call(&r, "GET", &format!("/_alias/{alias}"), None));
        let mut names: Vec<String> = got.as_object().expect("object").keys().cloned().collect();
        names.sort();
        names
    };
    assert_eq!(held("reads"), ["app-000002"]);
    assert_eq!(held("latest"), ["app-000002"]);
    assert_eq!(held("app"), ["app-000001", "app-000002"]);
    // What cannot be added is refused before anything is made.
    let before = json(&call(&r, "GET", "/_alias/app", None));
    for (bad, status) in [
        (r#"{"aliases":{"x":{"filter":{"match_all":{}}}}}"#, 501),
        (r#"{"aliases":{"a*":{}}}"#, 501),
        (r#"{"settings":{"index":{"bogus":1}}}"#, 501),
    ] {
        let reply = call(&r, "POST", "/app/_rollover", Some(bad));
        assert_eq!(reply.status, status, "{bad}: {}", reply.text());
    }
    assert_eq!(json(&call(&r, "GET", "/_alias/app", None)), before);
    assert_eq!(
        call(&r, "HEAD", "/app-000003", None).status,
        404,
        "a refused body created nothing"
    );
    let latest = json(&call(&r, "GET", "/_alias/latest", None));
    assert_eq!(
        latest["app-000002"]["aliases"]["latest"]["is_write_index"],
        true
    );
    // A name that is an index cannot be an alias: the roll is published, the
    // other names are still added, and the error names the new index.
    assert_eq!(call(&r, "PUT", "/taken", None).status, 200);
    let partial = call(
        &r,
        "POST",
        "/app/_rollover",
        Some(r#"{"aliases":{"taken":{},"after":{}}}"#),
    );
    assert_eq!(partial.status, 400, "{}", partial.text());
    assert!(partial.text().contains("app-000003"), "{}", partial.text());
    assert_eq!(
        json(&call(&r, "GET", "/_alias/after", None)),
        serde_json::json!({"app-000003": {"aliases": {"after": {}}}})
    );
    assert!(json(&call(&r, "GET", "/_alias/app", None))["app-000003"].is_object());
}

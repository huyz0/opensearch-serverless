//! docs/specs/snapshot-global-state AC-5 to AC-9: a take records the global
//! state it is asked for, behind `serverless.snapshots.take.capture_global_state`.

use crate::snapshot_take::{Rig, index, publish, repository, requests, send};
use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator};
use oss_format::{
    DocKind, GlobalState, MAX_DOC_BYTES, SnapshotState, StateSet, snapshot_state_key,
};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_meta::{ClusterConfig, DeleteMode, DescriptorStore, DocSet};
use oss_node::snapshot_route::{SnapshotService, routes};
use oss_node::snapshot_take_route::SnapshotTakeService;
use oss_rest::{Router, Server};
use oss_testkit::budget;

fn rig(capture: bool) -> Rig {
    let config = BudgetConfig::new(256 << 20)
        .with_pool(Pool::RequestBodies, 64 << 20)
        .expect("fits");
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let clock: oss_meta::PitClock = Arc::new(|| 1_700_000_000_000);
    let service = Arc::new(SnapshotService::new(
        Arc::clone(&blobs),
        "",
        "node-1",
        Arc::clone(&clock),
    ));
    let descriptors =
        DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).unwrap();
    let taker = Arc::new(
        SnapshotTakeService::new(
            Arc::clone(&blobs),
            "",
            DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).unwrap(),
            clock,
        )
        .with_capture_global_state(capture),
    );
    let router = routes(Router::new(), &service, Some(&taker)).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&config);
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig {
        server,
        store,
        descriptors,
    }
}

fn blobs(r: &Rig) -> Arc<dyn BlobStore> {
    Arc::clone(&r.store) as Arc<dyn BlobStore>
}

fn put_doc(r: &Rig, kind: DocKind, name: &str, body: &str) {
    let docs = DocSet::new(blobs(r), "", kind).unwrap();
    docs.put(name, body.as_bytes(), false, &mut vec![0u8; 1 << 19])
        .unwrap();
}

fn seeded() -> Rig {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "u1", 1);
    publish(&r, "logs", "u1", 0);
    ClusterConfig::new(blobs(&r), "")
        .unwrap()
        .update(
            &[("cluster.a", Some("1")), ("cluster.b", Some("true"))],
            &mut vec![0u8; 1 << 20],
        )
        .unwrap();
    put_doc(
        &r,
        DocKind::IndexTemplate,
        "t",
        r#"{"index_patterns":["x-*"]}"#,
    );
    put_doc(
        &r,
        DocKind::StoredScript,
        "s",
        r#"{"script":{"lang":"painless","source":"1"}}"#,
    );
    r
}

fn take(r: &Rig, name: &str, body: &str) -> support::Reply {
    send(
        r,
        "PUT",
        &format!("/_snapshot/backups/{name}?wait_for_completion=true"),
        Some(body),
    )
}

fn state_of(r: &Rig, name: &str) -> Option<GlobalState> {
    let key = snapshot_state_key("", "backups", name).unwrap();
    let mut buf = vec![0u8; 16 << 20];
    let read = r.store.read_register(&key, &mut buf).unwrap().unwrap();
    let SnapshotState::Held { record, .. } = SnapshotState::parse(&buf[..read.len]).unwrap() else {
        panic!("held");
    };
    GlobalState::find(record).unwrap()
}

// AC-5: with the setting on, a take that asks records the state and says so.
#[test]
fn a_take_that_asks_records_the_settings_and_every_set() {
    let r = seeded();
    let put = take(&r, "g", r#"{"indices":"logs","include_global_state":true}"#);
    assert_eq!(put.status, 200, "{}", put.text());
    assert!(
        put.text().contains(r#""include_global_state":true"#),
        "{}",
        put.text()
    );
    let state = state_of(&r, "g").expect("recorded");
    assert_eq!(state.settings.len(), 2);
    assert_eq!(state.documents(StateSet::IndexTemplates).len(), 1);
    assert_eq!(state.documents(StateSet::StoredScripts).len(), 1);
    assert!(state.documents(StateSet::IngestPipelines).is_empty());
    let got = send(&r, "GET", "/_snapshot/backups/g", None).text();
    assert!(got.contains(r#""include_global_state":true"#), "{got}");
}

// AC-6: not asking, or the setting off, records nothing and says false.
#[test]
fn without_the_flag_or_the_setting_nothing_is_recorded_and_the_answer_says_false() {
    let on = seeded();
    let plain = take(&on, "a", r#"{"indices":"logs"}"#);
    assert!(
        plain.text().contains(r#""include_global_state":false"#),
        "{}",
        plain.text()
    );
    assert!(state_of(&on, "a").is_none());

    let off = rig(false);
    repository(&off, true);
    index(&off, "logs", "u1", 1);
    publish(&off, "logs", "u1", 0);
    let asked = take(
        &off,
        "b",
        r#"{"indices":"logs","include_global_state":true}"#,
    );
    assert_eq!(asked.status, 200, "{}", asked.text());
    assert!(
        asked.text().contains(r#""include_global_state":false"#),
        "{}",
        asked.text()
    );
    assert!(state_of(&off, "b").is_none());
}

// AC-7: an empty state writes no member but the take still succeeds.
#[test]
fn an_empty_state_writes_no_member() {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "u1", 1);
    publish(&r, "logs", "u1", 0);
    let put = take(&r, "e", r#"{"indices":"logs","include_global_state":true}"#);
    assert_eq!(put.status, 200, "{}", put.text());
    assert!(state_of(&r, "e").is_none());
}

// AC-8: a state past 4 MiB is refused before any shard work, naming the set.
#[test]
fn a_state_past_the_limit_is_refused_naming_the_set_and_costs_no_shard_read() {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "u1", 1);
    publish(&r, "logs", "u1", 0);
    let big = format!(r#"{{"a":"{}"}}"#, "x".repeat(MAX_DOC_BYTES - 16));
    for n in 0..5 {
        put_doc(&r, DocKind::StoredScript, &format!("s{n}"), &big);
    }
    let put = take(
        &r,
        "big",
        r#"{"indices":"logs","include_global_state":true}"#,
    );
    assert_eq!(put.status, 400, "{}", put.text());
    assert!(put.text().contains("snapshot_too_large"), "{}", put.text());
    assert!(put.text().contains("stored_scripts"), "{}", put.text());
    let none = send(&r, "GET", "/_snapshot/backups/big", None);
    assert_eq!(none.status, 404, "{}", none.text());
}

// AC-9: what a take with state costs: the take without it, plus the state's reads.
#[test]
fn a_take_with_state_costs_what_the_budget_holds() {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "u1", 1);
    publish(&r, "logs", "u1", 0);
    let before = requests(&r);
    let put = take(&r, "c", r#"{"indices":"logs","include_global_state":true}"#);
    assert_eq!(put.status, 200, "{}", put.text());
    budget::check(
        "snapshot.take_global_state_empty.requests",
        requests(&r) - before,
        "a one-shard take with an empty global state",
    );
    seeded_docs(&r);
    let before = requests(&r);
    let put = take(&r, "d", r#"{"indices":"logs","include_global_state":true}"#);
    assert_eq!(put.status, 200, "{}", put.text());
    budget::check(
        "snapshot.take_global_state_one_doc.requests",
        requests(&r) - before,
        "the same take with one template and one script",
    );
}

fn seeded_docs(r: &Rig) {
    put_doc(
        r,
        DocKind::IndexTemplate,
        "t",
        r#"{"index_patterns":["x-*"]}"#,
    );
    put_doc(
        r,
        DocKind::StoredScript,
        "s",
        r#"{"script":{"lang":"painless","source":"1"}}"#,
    );
}

/// The record a Rust take wrote with every part of the global state, for a
/// Java node to read (`fixtures/snapshots/rust-take-global/`,
/// `ServerlessRustGlobalStateTests.java`). With `OSS_WRITE_FIXTURES` set the
/// file is rewritten; otherwise the test checks the committed one still
/// holds the state this take records.
#[test]
#[allow(clippy::disallowed_methods)]
fn a_take_writes_the_global_state_record_a_java_node_reads() {
    let r = rig(true);
    repository(&r, true);
    index(&r, "logs", "u1", 1);
    publish(&r, "logs", "u1", 0);
    ClusterConfig::new(blobs(&r), "")
        .unwrap()
        .update(
            &[
                ("cluster.int", Some("7")),
                ("cluster.text", Some("a b")),
                ("cluster.flag", Some("true")),
                ("cluster.ratio", Some("1.5")),
            ],
            &mut vec![0u8; 1 << 20],
        )
        .unwrap();
    put_doc(
        &r,
        DocKind::IndexTemplate,
        "it",
        r#"{"index_patterns":["x-*"],"template":{"settings":{"index":{"number_of_shards":"2"}}}}"#,
    );
    put_doc(
        &r,
        DocKind::ComponentTemplate,
        "ct",
        r#"{"template":{"mappings":{"properties":{"a":{"type":"keyword"}}}}}"#,
    );
    put_doc(
        &r,
        DocKind::IngestPipeline,
        "ip",
        r#"{"processors":[{"set":{"field":"a","value":"é \"q\""}}]}"#,
    );
    put_doc(
        &r,
        DocKind::SearchPipeline,
        "sp",
        r#"{"request_processors":[{"filter_query":{"query":{"match_all":{}}}}]}"#,
    );
    put_doc(
        &r,
        DocKind::StoredScript,
        "ss",
        r#"{"script":{"lang":"painless","source":"1"}}"#,
    );
    let put = take(&r, "g", r#"{"indices":"logs","include_global_state":true}"#);
    assert_eq!(put.status, 200, "{}", put.text());
    let key = snapshot_state_key("", "backups", "g").unwrap();
    let mut state = vec![0u8; 1 << 20];
    let read = r.store.read_register(&key, &mut state).unwrap().unwrap();
    state.truncate(read.len);
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/snapshots/rust-take-global");
    if std::env::var_os("OSS_WRITE_FIXTURES").is_some() {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("state.bin"), &state).unwrap();
    }
    let committed = std::fs::read(dir.join("state.bin")).expect("run with OSS_WRITE_FIXTURES=1");
    let SnapshotState::Held { record, .. } = SnapshotState::parse(&committed).unwrap() else {
        panic!("held");
    };
    let held = GlobalState::find(record).unwrap().expect("a member");
    assert_eq!(held.settings.len(), 4);
    for set in StateSet::ALL {
        assert_eq!(held.documents(set).len(), 1, "{}", set.key());
    }
}

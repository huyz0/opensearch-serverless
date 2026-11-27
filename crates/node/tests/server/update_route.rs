//! docs/specs/node-update-route: the update API over a writer node's own
//! shards.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_format::{NodeLogEpoch, ShardKey};
use oss_mem::MemoryBudget;
use oss_node::doc_route::{DocService, routes};
use oss_node::update_route::{UpdateService, routes as update_routes};
use oss_node::writer::{Routes, WriterRole};
use oss_rest::{Router, Server};
use oss_writer::bulk::{IndexInfo, ItemResult, Owner, Planned};
use oss_writer::log::{
    CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, Refusal, ShardAdmission,
};
use oss_writer::mapping::Mapping;

struct Lease;

impl EpochLease for Lease {
    fn record(&self, _: &[NodeLogEpoch]) -> Result<(), Refusal> {
        Ok(())
    }
}

struct Admit;

impl ShardAdmission for Admit {
    fn admit(&self, _: &ShardKey, _: i64, _: u64) -> Result<(), Refusal> {
        Ok(())
    }
}

/// One index, `docs`, of one shard held here; any other name is unknown.
struct Local;

fn info() -> IndexInfo {
    IndexInfo {
        name: "docs".into(),
        uuid: "docs-uuid".into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
        write_block: Default::default(),
        default_pipeline: None,
        final_pipeline: None,
    }
}

impl Routes for Local {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        (name == "docs").then(info)
    }

    fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn forward(&self, _: &str, _: &IndexInfo, _: u32, group: &[Planned<'_>]) -> Vec<ItemResult> {
        unreachable!("every shard is local: {}", group.len())
    }
}

struct Rig {
    server: Server,
}

fn rig() -> Rig {
    let config = support::budget_config();
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let log = NodeLogStore::new(store as Arc<dyn BlobStore>, "", "node-1").expect("names");
    let committer = Arc::new(NodeLogCommitter::new(
        log,
        7,
        Box::new(Arc::new(Lease)),
        Box::new(Arc::new(Admit)),
        CommitterConfig {
            window: Duration::from_millis(0),
            ..CommitterConfig::default()
        },
    ));
    let role = Arc::new(WriterRole::new(committer, Arc::new(Local)));
    role.open_shard(
        &info(),
        0,
        3,
        Mapping::parse(br#"{"properties":{"n":{"type":"integer"},"m":{"type":"integer"},"o":{"properties":{"x":{"type":"integer"},"y":{"type":"integer"}}}}}"#).expect("mapping"),
    )
    .expect("opens");
    let service = Arc::new(DocService::new(Arc::clone(&role), [0; 6]));
    let router = routes(Router::new(), service).expect("routes");
    let router =
        update_routes(router, Arc::new(UpdateService::new(role, [0; 6]))).expect("update routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server = Server::start(
        listener,
        support::server_config(),
        router,
        &MemoryBudget::new(&config),
    )
    .expect("starts");
    Rig { server }
}

fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

fn put(r: &Rig, id: &str, source: &str) {
    let w = call(r, "PUT", &format!("/docs/_doc/{id}"), Some(source));
    assert!(w.status == 200 || w.status == 201, "{}", w.text());
}

fn update(r: &Rig, id: &str, query: &str, body: &str) -> support::Reply {
    call(r, "POST", &format!("/docs/_update/{id}{query}"), Some(body))
}

#[test]
fn a_doc_merges_into_the_stored_document() {
    let r = rig();
    put(&r, "1", r#"{"n":1,"o":{"x":1,"y":2}}"#);
    let u = update(&r, "1", "", r#"{"doc":{"m":5,"o":{"y":9}}}"#);
    let t = u.text();
    assert_eq!(u.status, 200, "{t}");
    assert!(t.contains(r#""_version":2,"result":"updated""#), "{t}");
    // an unchanged merge is a noop, which shows the stored source is the merged one
    let same = update(&r, "1", "", r#"{"doc":{"n":1,"m":5,"o":{"x":1,"y":9}}}"#);
    let t = same.text();
    assert_eq!(same.status, 200, "{t}");
    assert!(t.contains(r#""result":"noop""#), "{t}");
    assert!(t.contains(r#""_version":2"#), "{t}");
    assert!(
        t.contains(r#""_shards":{"total":0,"successful":0,"failed":0}"#),
        "{t}"
    );
    let other = update(&r, "1", "", r#"{"doc":{"n":1,"m":5,"o":{"x":1,"y":8}}}"#);
    assert!(
        other.text().contains(r#""_version":3,"result":"updated""#),
        "{}",
        other.text()
    );
}

#[test]
fn detect_noop_false_writes_an_unchanged_merge() {
    let r = rig();
    put(&r, "1", r#"{"n":1}"#);
    let u = update(&r, "1", "", r#"{"doc":{"n":1},"detect_noop":false}"#);
    assert!(
        u.text().contains(r#""_version":2,"result":"updated""#),
        "{}",
        u.text()
    );
}

#[test]
fn a_missing_document_is_refused_or_upserted() {
    let r = rig();
    let missing = update(&r, "7", "", r#"{"doc":{"n":1}}"#);
    assert_eq!(missing.status, 404, "{}", missing.text());
    assert!(missing.text().contains("document_missing_exception"));
    let as_upsert = update(&r, "7", "", r#"{"doc":{"n":1},"doc_as_upsert":true}"#);
    assert_eq!(as_upsert.status, 201, "{}", as_upsert.text());
    assert!(as_upsert.text().contains(r#""result":"created""#));
    let upsert = update(&r, "8", "", r#"{"doc":{"n":1},"upsert":{"n":2}}"#);
    assert_eq!(upsert.status, 201, "{}", upsert.text());
    let after = update(&r, "8", "", r#"{"doc":{"n":2}}"#);
    assert!(
        after.text().contains(r#""result":"noop""#),
        "the upsert wrote n=2: {}",
        after.text()
    );
}

#[test]
fn the_callers_condition_and_the_options_are_checked() {
    let r = rig();
    put(&r, "1", r#"{"n":1}"#);
    let stale = update(
        &r,
        "1",
        "?if_seq_no=99&if_primary_term=3",
        r#"{"doc":{"n":2}}"#,
    );
    assert_eq!(stale.status, 409, "{}", stale.text());
    assert!(stale.text().contains("version_conflict_engine_exception"));
    for (query, body, needle) in [
        (
            "?retry_on_conflict=-1",
            r#"{"doc":{"n":2}}"#,
            "retry_on_conflict must be non negative",
        ),
        (
            "",
            r#"{"doc":{"n":2},"scripted_upsert":true}"#,
            "scripted_upsert was given without a script",
        ),
        ("", r#"{"doc":{"n":2},"bogus":1}"#, "unknown field [bogus]"),
        ("?bogus=1", r#"{"doc":{"n":2}}"#, "bogus"),
    ] {
        let u = update(&r, "1", query, body);
        assert!(u.status >= 400, "{query}: {}", u.text());
        assert!(u.text().contains(needle), "{query}: {}", u.text());
    }
    assert_eq!(
        update(&r, "1", "?require_alias=true", r#"{"doc":{"n":2}}"#).status,
        501
    );
    assert_eq!(update(&r, "1", "", "[]").status, 400);
}

#[test]
fn an_update_script_edits_deletes_or_skips_behind_the_switch() {
    let r = rig();
    put(&r, "1", r#"{"n":1,"o":{"x":1}}"#);
    let script = |s: &str| format!(r#"{{"script":{{"source":"{s}","params":{{"by":4}}}}}}"#);
    oss_reader::painless_switch::set_native_compile(false);
    let off = update(&r, "1", "", &script("ctx._source.n += params.by"));
    assert_eq!(off.status, 501, "{}", off.text());
    oss_reader::painless_switch::set_native_compile(true);
    let edited = update(
        &r,
        "1",
        "",
        &script("ctx._source.n += params.by; ctx._source.o.y = 7"),
    );
    assert!(
        edited.text().contains(r#""_version":2,"result":"updated""#),
        "{}",
        edited.text()
    );
    let check = update(&r, "1", "", r#"{"doc":{"n":5,"o":{"x":1,"y":7}}}"#);
    assert!(
        check.text().contains(r#""result":"noop""#),
        "script left n=5,o.y=7: {}",
        check.text()
    );
    let skip = update(&r, "1", "", &script("ctx.op = 'noop'"));
    assert!(
        skip.text().contains(r#""result":"noop""#),
        "{}",
        skip.text()
    );
    let bad = update(&r, "1", "", &script("ctx._source.n = "));
    assert_eq!(bad.status, 400, "{}", bad.text());
    assert!(bad.text().contains("script_exception"));
    let upsert = update(
        &r,
        "2",
        "",
        r#"{"script":{"source":"ctx._source.n = 9"},"scripted_upsert":true,"upsert":{"m":1}}"#,
    );
    assert_eq!(upsert.status, 201, "{}", upsert.text());
    let gone = update(&r, "1", "", &script("ctx.op = 'delete'"));
    assert!(
        gone.text().contains(r#""result":"deleted""#),
        "{}",
        gone.text()
    );
    assert_eq!(update(&r, "1", "", r#"{"doc":{"n":1}}"#).status, 404);
    oss_reader::painless_switch::set_native_compile(false);
}

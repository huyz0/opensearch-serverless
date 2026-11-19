//! docs/specs/node-doc-write-route: the single-document write APIs over a
//! writer node's own shards.

#[path = "support/mod.rs"]
mod support;

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_format::{NodeLogEpoch, ShardKey};
use oss_mem::MemoryBudget;
use oss_node::doc_route::{DocService, routes};
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
        Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping"),
    )
    .expect("opens");
    let service = Arc::new(DocService::new(role, [0; 6]));
    let router = routes(Router::new(), service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server = Server::start(
        listener,
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
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

#[test]
fn a_document_is_created_then_updated() {
    let r = rig();
    for method in ["PUT", "POST"] {
        let first = call(
            &r,
            method,
            &format!("/docs/_doc/{method}"),
            Some(r#"{"n":1}"#),
        );
        let text = first.text();
        assert_eq!(first.status, 201, "{text}");
        assert!(text.contains(r#""_index":"docs","_id":""#), "{text}");
        assert!(
            text.contains(r#""_version":1,"result":"created""#),
            "{text}"
        );
        assert!(
            text.contains(r#""_shards":{"total":1,"successful":1,"failed":0}"#),
            "{text}"
        );
        assert!(text.contains(r#""_seq_no":"#), "{text}");
        assert!(text.contains(r#""_primary_term":3"#), "{text}");
        let second = call(
            &r,
            method,
            &format!("/docs/_doc/{method}"),
            Some(r#"{"n":2}"#),
        );
        let text = second.text();
        assert_eq!(second.status, 200, "{text}");
        assert!(
            text.contains(r#""_version":2,"result":"updated""#),
            "{text}"
        );
    }
}

#[test]
fn a_post_without_an_id_generates_one() {
    let r = rig();
    let a = call(&r, "POST", "/docs/_doc", Some(r#"{"n":1}"#));
    let b = call(&r, "POST", "/docs/_doc", Some(r#"{"n":1}"#));
    assert_eq!((a.status, b.status), (201, 201), "{}", a.text());
    let id = |t: String| {
        let at = t.find(r#""_id":""#).expect("id") + 7;
        t[at..].split('"').next().expect("quoted").to_owned()
    };
    let (ia, ib) = (id(a.text()), id(b.text()));
    assert!(!ia.is_empty() && ia != ib, "{ia} {ib}");
}

#[test]
fn create_refuses_a_live_id() {
    let r = rig();
    assert_eq!(
        call(&r, "PUT", "/docs/_create/1", Some(r#"{"n":1}"#)).status,
        201
    );
    let again = call(&r, "PUT", "/docs/_create/1", Some(r#"{"n":1}"#));
    assert_eq!(again.status, 409, "{}", again.text());
    assert!(again.text().contains("version_conflict_engine_exception"));
    assert!(again.text().contains(r#""status":409"#));
    let by_type = call(&r, "PUT", "/docs/_doc/1?op_type=create", Some(r#"{"n":1}"#));
    assert_eq!(by_type.status, 409, "{}", by_type.text());
}

#[test]
fn delete_answers_deleted_then_not_found() {
    let r = rig();
    call(&r, "PUT", "/docs/_doc/1", Some(r#"{"n":1}"#));
    let gone = call(&r, "DELETE", "/docs/_doc/1", None);
    assert_eq!(gone.status, 200, "{}", gone.text());
    assert!(
        gone.text().contains(r#""result":"deleted""#),
        "{}",
        gone.text()
    );
    let none = call(&r, "DELETE", "/docs/_doc/never", None);
    assert_eq!(none.status, 404, "{}", none.text());
    let text = none.text();
    assert!(text.contains(r#""result":"not_found""#), "{text}");
    assert!(text.contains(r#""_seq_no":"#), "{text}");
}

#[test]
fn refresh_and_if_seq_no_follow_their_parameters() {
    let r = rig();
    let text = call(&r, "PUT", "/docs/_doc/1?refresh=true", Some(r#"{"n":1}"#)).text();
    assert!(text.contains(r#""forced_refresh":true"#), "{text}");
    let text = call(&r, "PUT", "/docs/_doc/2?refresh=false", Some(r#"{"n":1}"#)).text();
    assert!(!text.contains("forced_refresh"), "{text}");
    let bad = call(&r, "PUT", "/docs/_doc/3?refresh=maybe", Some(r#"{"n":1}"#));
    assert_eq!(bad.status, 400, "{}", bad.text());
    let stale = call(
        &r,
        "PUT",
        "/docs/_doc/1?if_seq_no=99&if_primary_term=3",
        Some(r#"{"n":2}"#),
    );
    assert_eq!(stale.status, 409, "{}", stale.text());
    let not_a_number = call(&r, "PUT", "/docs/_doc/1?if_seq_no=abc", Some(r#"{"n":2}"#));
    assert_eq!(not_a_number.status, 400, "{}", not_a_number.text());
}

#[test]
fn a_refused_document_and_an_unknown_index_answer_their_failure() {
    let r = rig();
    let refused = call(&r, "PUT", "/docs/_doc/1", Some(r#"{"n":"abc"}"#));
    assert_eq!(refused.status, 400, "{}", refused.text());
    let text = refused.text();
    assert!(text.starts_with(r#"{"error":{"#), "{text}");
    assert!(text.ends_with(r#""status":400}"#), "{text}");
    let unknown = call(&r, "PUT", "/nope/_doc/1", Some(r#"{"n":1}"#));
    assert!(unknown.status >= 400, "{}", unknown.text());
    assert!(unknown.text().contains(r#""error":"#), "{}", unknown.text());
}

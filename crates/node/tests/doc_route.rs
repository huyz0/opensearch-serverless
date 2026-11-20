//! docs/specs/node-doc-write-route: the single-document write APIs over a
//! writer node's own shards.

#[path = "support/mod.rs"]
mod support;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_format::{NodeLogEpoch, ShardKey};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
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
    rig_with(&support::budget_config())
}

/// A node whose request-body pool holds `bytes`.
fn rig_pooled(bytes: usize) -> Rig {
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::RequestBodies, bytes)
        .expect("fits");
    rig_with(&config)
}

fn rig_with(config: &BudgetConfig) -> Rig {
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
        &MemoryBudget::new(config),
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

/// The status of a `PUT` whose body goes as `sizes`-byte chunks, with no
/// declared length.
fn chunked_status(r: &Rig, target: &str, chunks: &[&[u8]]) -> u16 {
    let mut s = TcpStream::connect(r.server.local_addr()).expect("connects");
    s.set_read_timeout(Some(Duration::from_secs(30)))
        .expect("timeout");
    let head = format!(
        "PUT {target} HTTP/1.1\r\nhost: test\r\nconnection: close\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n"
    );
    s.write_all(head.as_bytes()).expect("sends head");
    for chunk in chunks {
        let _ = s.write_all(format!("{:x}\r\n", chunk.len()).as_bytes());
        let _ = s.write_all(chunk);
        let _ = s.write_all(b"\r\n");
    }
    let _ = s.write_all(b"0\r\n\r\n");
    let mut raw = Vec::new();
    let mut buf = [0u8; 4 << 10];
    while let Ok(n) = s.read(&mut buf) {
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }
    String::from_utf8_lossy(&raw)
        .split(' ')
        .nth(1)
        .and_then(|v| v.parse().ok())
        .expect("a status")
}

/// What the server itself holds of the request-body pool once started.
const SERVER_HELD: usize = 2 * 81_936 + 65_536 + 262_144;

/// A JSON document of exactly `len` bytes.
fn document_of(len: usize) -> String {
    let pad = len.saturating_sub(r#"{"s":""}"#.len());
    format!(r#"{{"s":"{}"}}"#, "x".repeat(pad))
}

/// The millisecond clock a generated id carries: bytes 2 to 5 are bits 16 to
/// 47 of the timestamp, byte 12 bits 8 to 15 and byte 14 bits 0 to 7.
// ARITH: four 6-bit values make 24 bits, inside a u32.
#[allow(clippy::arithmetic_side_effects)]
fn stamp(id: &str) -> u64 {
    let value = |c: u8| -> u32 {
        let at = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
            .iter()
            .position(|a| *a == c)
            .expect("a base64url character");
        u32::try_from(at).expect("under 64")
    };
    let mut bytes = Vec::new();
    for quad in id.as_bytes().chunks(4) {
        let n = quad.iter().fold(0u32, |n, c| (n << 6) | value(*c));
        let [_, a, b, c] = n.to_be_bytes();
        bytes.extend([a, b, c]);
    }
    u64::from_be_bytes([
        0, 0, bytes[5], bytes[4], bytes[3], bytes[2], bytes[12], bytes[14],
    ])
}

#[test]
fn a_generated_id_carries_the_time_of_the_write() {
    let r = rig();
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch")
        .as_millis();
    let text = call(&r, "POST", "/docs/_doc", Some(r#"{"n":1}"#)).text();
    let id = text
        .split(r#""_id":""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("an id");
    let millis = u128::from(stamp(id));
    assert!(
        millis >= before && millis < before.saturating_add(60_000),
        "id {id} is stamped {millis}, the write began at {before}"
    );
}

#[test]
fn op_type_create_refuses_a_live_id() {
    let r = rig();
    let first = call(&r, "PUT", "/docs/_doc/1?op_type=create", Some(r#"{"n":1}"#));
    assert_eq!(first.status, 201, "{}", first.text());
    let again = call(&r, "PUT", "/docs/_doc/1?op_type=create", Some(r#"{"n":2}"#));
    assert_eq!(again.status, 409, "{}", again.text());
    let index = call(&r, "PUT", "/docs/_doc/1?op_type=index", Some(r#"{"n":2}"#));
    assert_eq!(index.status, 200, "{}", index.text());
}

#[test]
fn a_document_is_charged_to_the_request_body_pool_once() {
    let body = document_of(8 << 10);
    // A pool with room for the document and a little over takes it, so the
    // document is charged once and not again as it is read.
    let r = rig_pooled(SERVER_HELD + (8 << 10) + 1024);
    let ok = call(&r, "PUT", "/docs/_doc/1", Some(&body));
    assert_eq!(ok.status, 201, "{}", ok.text());
}

#[test]
fn a_document_with_no_declared_length_is_charged_as_it_is_read() {
    let body = document_of(8 << 10);
    let (a, b) = body.as_bytes().split_at(4 << 10);
    let small = rig_pooled(SERVER_HELD + (2 << 10));
    assert_eq!(chunked_status(&small, "/docs/_doc/1", &[a, b]), 429);
    let large = rig_pooled(SERVER_HELD + (1 << 20));
    assert_eq!(chunked_status(&large, "/docs/_doc/1", &[a, b]), 201);
}

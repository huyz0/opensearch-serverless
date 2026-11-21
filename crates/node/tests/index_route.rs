//! docs/specs/node-index-route: create an index and ask whether one exists.

#[path = "support/mod.rs"]
mod support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore, PitClock};
use oss_node::index_route::{IndexService, routes};
use oss_rest::{Router, Server};

const BUF: usize = 1 << 16;
const NOW: u64 = 1_234_567;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
    descriptors: DescriptorStore,
}

fn rig_with(budget: &MemoryBudget, buffer: usize) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let clock: PitClock = Arc::new(|| NOW);
    let descriptors = || {
        DescriptorStore::new(
            Arc::clone(&store) as Arc<dyn BlobStore>,
            "",
            DeleteMode::Conditional,
        )
        .expect("names")
    };
    let service = Arc::new(IndexService::new(descriptors(), clock, buffer));
    let router = routes(Router::new(), service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server = Server::start(listener, support::server_config(), router, budget).expect("starts");
    Rig {
        server,
        descriptors: descriptors(),
        store,
    }
}

fn rig() -> Rig {
    rig_with(&MemoryBudget::new(&support::budget_config()), BUF)
}

fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

fn ops(r: &Rig) -> Vec<(S3Op, u64)> {
    S3Op::ALL
        .iter()
        .map(|op| (*op, r.store.meter().count(*op)))
        .filter(|(_, n)| *n > 0)
        .collect()
}

fn record(r: &Rig, name: &str) -> Option<(String, u32, Option<String>, i64)> {
    let mut buf = vec![0u8; BUF];
    r.descriptors.get(name, &mut buf).expect("read").map(|rec| {
        (
            rec.uuid.to_string(),
            rec.number_of_shards,
            rec.mapping.as_deref().map(str::to_owned),
            rec.created_at_millis,
        )
    })
}

const MAPPING: &str = r#"{"properties":{"title":{"type":"text"}}}"#;

#[test]
fn a_create_writes_the_descriptor_and_acknowledges() {
    let r = rig();
    let body = format!(
        r#"{{"settings":{{"number_of_shards":3,"number_of_replicas":0}},"mappings":{MAPPING}}}"#
    );
    let reply = call(&r, "PUT", "/logs", Some(&body));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(
        reply.text(),
        r#"{"acknowledged":true,"shards_acknowledged":true,"index":"logs","shards":3}"#
    );
    let (uuid, shards, mapping, created) = record(&r, "logs").expect("created");
    assert_eq!(uuid.len(), 22);
    assert_eq!(shards, 3);
    assert_eq!(mapping.as_deref(), Some(MAPPING));
    assert_eq!(created, i64::try_from(NOW).expect("small"));
    // A body that is only a mapping, and none at all.
    assert_eq!(call(&r, "PUT", "/bare", Some(MAPPING)).status, 200);
    assert_eq!(
        record(&r, "bare").expect("created").2.as_deref(),
        Some(MAPPING)
    );
    assert_eq!(call(&r, "PUT", "/empty", None).status, 200);
    assert_eq!(record(&r, "empty").expect("created").1, 1);
}

#[test]
fn a_second_create_of_a_name_is_refused_and_changes_nothing() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/logs", Some(MAPPING)).status, 200);
    let first = record(&r, "logs").expect("created");
    let reply = call(&r, "PUT", "/logs", None);
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(reply.text().contains("resource_already_exists_exception"));
    assert_eq!(record(&r, "logs"), Some(first));
}

#[test]
fn a_bad_name_is_refused_before_any_request() {
    let r = rig();
    for (name, status) in [("/_all", 501), ("/.ds-x", 400), ("/Upper", 400)] {
        let reply = call(&r, "PUT", name, None);
        assert_eq!(reply.status, status, "{name}: {}", reply.text());
    }
    assert!(
        call(&r, "PUT", "/Upper", None)
            .text()
            .contains("invalid_index_name_exception")
    );
    assert!(ops(&r).is_empty(), "{:?}", ops(&r));
}

#[test]
fn what_cannot_be_created_writes_nothing() {
    let r = rig();
    for (name, body, status) in [
        ("a", r#"{"settings":{"refresh_interval":"1s"}}"#, 501),
        ("b", r#"{"aliases":{"x":{}}}"#, 501),
        ("c", "[1]", 400),
        ("d", r#"{"settings":{"number_of_shards":0}}"#, 400),
        (
            "e",
            r#"{"mappings":{"properties":{"f":{"type":"nonsense"}}}}"#,
            400,
        ),
    ] {
        let reply = call(&r, "PUT", &format!("/{name}"), Some(body));
        assert_eq!(reply.status, status, "{name}: {}", reply.text());
        assert!(record(&r, name).is_none(), "{name}");
    }
    assert!(
        ops(&r).iter().all(|(op, _)| *op != S3Op::Put),
        "{:?}",
        ops(&r)
    );
}

#[test]
fn head_says_whether_a_name_holds_an_index_or_an_alias() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/logs", None).status, 200);
    let reply = call(&r, "HEAD", "/logs", None);
    assert_eq!(reply.status, 200);
    assert!(reply.body.is_empty());
    let reply = call(&r, "HEAD", "/nothing", None);
    assert_eq!(reply.status, 404);
    assert!(reply.body.is_empty());
    let alias = oss_format::AliasRecord::new("a", &["logs"]);
    let mut buf = vec![0u8; BUF];
    r.descriptors
        .create_alias(&alias, 1, &mut buf)
        .expect("alias");
    assert_eq!(call(&r, "HEAD", "/a", None).status, 200);
}

#[test]
fn a_create_is_one_conditional_create_and_a_head_one_read() {
    let r = rig();
    let before = ops(&r);
    assert_eq!(call(&r, "PUT", "/logs", None).status, 200);
    let created: u64 =
        ops(&r).iter().map(|(_, n)| n).sum::<u64>() - before.iter().map(|(_, n)| n).sum::<u64>();
    assert_eq!(created, 1, "{:?}", ops(&r));
    let mid: u64 = ops(&r).iter().map(|(_, n)| n).sum();
    assert_eq!(call(&r, "HEAD", "/logs", None).status, 200);
    let head: u64 = ops(&r).iter().map(|(_, n)| n).sum::<u64>() - mid;
    assert_eq!(head, 1, "{:?}", ops(&r));
    assert!(ops(&r).iter().all(|(op, _)| *op != S3Op::List));
}

#[test]
fn a_pool_without_room_for_the_buffer_answers_429_and_writes_nothing() {
    let r = rig_with(&MemoryBudget::new(&support::budget_config()), 64 << 20);
    let reply = call(&r, "PUT", "/logs", None);
    assert_eq!(reply.status, 429, "{}", reply.text());
    assert!(ops(&r).is_empty(), "{:?}", ops(&r));
}

#[test]
fn a_body_of_one_mebibyte_is_read_and_one_byte_more_is_refused() {
    let r = rig_with(&MemoryBudget::new(&support::budget_config()), 2 << 20);
    let mut exact = String::from(MAPPING);
    exact.push_str(&" ".repeat((1 << 20) - MAPPING.len()));
    assert_eq!(exact.len(), 1 << 20);
    let reply = call(&r, "PUT", "/exact", Some(&exact));
    assert_eq!(reply.status, 200, "{}", reply.text());
    let over = format!("{exact} ");
    let reply = call(&r, "PUT", "/over", Some(&over));
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(record(&r, "over").is_none());
}

#[test]
fn a_descriptor_that_does_not_fit_its_buffer_is_refused_not_failed() {
    let r = rig();
    let mut big = String::from(MAPPING);
    big.push_str(&" ".repeat(BUF));
    let reply = call(&r, "PUT", "/big", Some(&big));
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(record(&r, "big").is_none());
}

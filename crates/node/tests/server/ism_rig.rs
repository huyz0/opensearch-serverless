//! The rig the ISM route tests share: a server over the emulator, and helpers.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::IndexRecord;
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::ism_route::{IsmService, routes};
use oss_rest::{Router, Server};

pub(crate) const BUF: usize = 1 << 16;
pub(crate) const POLICY: &str = r#"{"policy":{"description":"logs","default_state":"hot","states":[
  {"name":"hot","actions":[{"rollover":{"min_index_age":"1d"}}],
   "transitions":[{"state_name":"warm","conditions":{"min_index_age":"2d"}}]},
  {"name":"warm","actions":[{"read_only":{}}],
   "transitions":[{"state_name":"gone","conditions":{"min_index_age":"30d"}}]},
  {"name":"gone","actions":[{"delete":{}}],"transitions":[]}]}}"#;

pub(crate) struct Rig {
    pub(crate) server: Server,
    pub(crate) store: Arc<Emulator>,
}

pub(crate) fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let descriptors =
        DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).expect("descriptors");
    let service = Arc::new(IsmService::new(&blobs, "", BUF).with_descriptors(descriptors));
    let router = routes(Router::new(), &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig { server, store }
}

pub(crate) fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

pub(crate) fn total(r: &Rig, op: S3Op) -> u64 {
    r.store.meter().count(op)
}

pub(crate) fn with(policy: &str, from: &str, to: &str) -> String {
    assert!(policy.contains(from), "{from}");
    policy.replacen(from, to, 1)
}

/// An index record as the coordinator writes one, created at `created`.
pub(crate) fn seed(r: &Rig, name: &str, created: i64) {
    let blobs = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let descriptors =
        DescriptorStore::new(blobs, "", DeleteMode::Conditional).expect("descriptors");
    let uuid = format!("uuid-{name}");
    let mut record = IndexRecord::new(name, &uuid, 1);
    record.created_at_millis = created;
    descriptors
        .create(&record, 1, &mut vec![0u8; BUF])
        .expect("index");
}

pub(crate) fn json(reply: &support::Reply) -> serde_json::Value {
    serde_json::from_str(&reply.text()).unwrap_or_else(|e| panic!("{e}: {}", reply.text()))
}

pub(crate) const ADD: &str = r#"{"policy_id":"logs"}"#;

pub(crate) fn policy_logs2() -> String {
    POLICY.replacen("\"logs\"", "\"logs2\"", 1)
}

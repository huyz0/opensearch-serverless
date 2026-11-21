//! A rig for the point-in-time route tests: a server over the emulator, the
//! stores it serves, and a metered call.

// Each test file uses part of the rig.
#![allow(dead_code)]

#[path = "../support/mod.rs"]
mod support;

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{FileLocation, IndexRecord, ManifestEntry, ShardRef, encode_manifest};
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore, PitClock, PointInTimeStore};
use oss_node::pit_route::{PitService, routes};
use oss_rest::{Router, Server};

pub const BUF: usize = 1 << 16;

pub struct Rig {
    pub server: Server,
    pub store: Arc<dyn BlobStore>,
    pub pits: PointInTimeStore,
    pub descriptors: DescriptorStore,
    pub now: Arc<AtomicU64>,
}

pub fn rig() -> Rig {
    rig_sized(BUF)
}

pub fn rig_sized(buffer_bytes: usize) -> Rig {
    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 28, 5));
    let now = Arc::new(AtomicU64::new(1_000_000));
    let read = Arc::clone(&now);
    let clock: PitClock = Arc::new(move || read.load(Ordering::SeqCst));
    let service = Arc::new(PitService::new(
        PointInTimeStore::new(Arc::clone(&store), "", Arc::clone(&clock)).expect("pits"),
        DescriptorStore::new(Arc::clone(&store), "", DeleteMode::Conditional).expect("names"),
        Arc::clone(&clock),
        buffer_bytes,
    ));
    let router = routes(Router::new(), service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server = Server::start(
        listener,
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    Rig {
        server,
        pits: PointInTimeStore::new(Arc::clone(&store), "", clock).expect("pits"),
        descriptors: DescriptorStore::new(Arc::clone(&store), "", DeleteMode::Conditional)
            .expect("names"),
        store,
        now,
    }
}

pub fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

/// An index of `shards` shards, each with a published commit.
pub fn index(r: &Rig, name: &str, shards: u32) {
    let record = IndexRecord::new(name, "u1", shards);
    r.descriptors
        .create(&record, 1, &mut vec![0u8; BUF])
        .expect("created");
    for shard in 0..shards {
        let at = ShardRef {
            index: name,
            uuid: "u1",
            shard,
        };
        let entries = [ManifestEntry {
            name: "_0.si",
            location: FileLocation::Whole { term_dir: "t=1" }.encode(),
            length: 7,
        }];
        let value = encode_manifest(3, Some("n1"), &entries, -1, -1).expect("manifest");
        r.store
            .create_register(&at.manifest_key("").expect("key"), &value)
            .expect("published");
    }
}

pub fn id_of(text: &str) -> String {
    let rest = text.split("\"pit_id\":\"").nth(1).expect("a pit_id");
    rest.split('"').next().expect("an id").to_owned()
}

pub fn create(r: &Rig, index: &str) -> String {
    let reply = call(r, "POST", &format!("/{index}/_search/point_in_time"), None);
    assert_eq!(reply.status, 200, "{}", reply.text());
    id_of(&reply.text())
}

/// The requests `work` sends to the store, per kind.
pub fn sent<T>(r: &Rig, work: impl FnOnce() -> T) -> (T, Vec<(S3Op, u64)>) {
    let meter = r.store.meter();
    let before = S3Op::ALL.map(|op| meter.count(op));
    let out = work();
    let added = S3Op::ALL
        .iter()
        .zip(before)
        .map(|(op, b)| (*op, meter.count(*op).saturating_sub(b)))
        .filter(|(_, n)| *n > 0)
        .collect();
    (out, added)
}

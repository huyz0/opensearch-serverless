//! A rig for the cluster route tests: a server over the emulator with the
//! settings, cat and health routes, a fake health source, and a metered call.

// Each test file uses part of the rig.
#![allow(dead_code)]

#[path = "../support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_coordinator::cluster::health::{HeadRead, IndexInput};
use oss_format::IndexRecord;
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::cat_route::{self, CatService};
use oss_node::cluster_route::{self, ClusterService};
use oss_node::health_route::{self, HealthService, HealthSource};
use oss_rest::{Router, Server};

pub const BUF: usize = 1 << 16;

/// What the fake says: a node count, and what each index's heads said.
#[derive(Default)]
pub struct Fake {
    pub nodes: AtomicUsize,
    pub unreadable: AtomicBool,
    pub indices: Mutex<HashMap<String, IndexInput>>,
}

impl HealthSource for Fake {
    fn nodes(&self) -> Option<usize> {
        (!self.unreadable.load(Ordering::SeqCst)).then(|| self.nodes.load(Ordering::SeqCst))
    }

    fn index(&self, name: &str) -> IndexInput {
        self.indices
            .lock()
            .expect("lock")
            .get(name)
            .cloned()
            .unwrap_or(IndexInput::Missing)
    }
}

pub struct Rig {
    pub server: Server,
    pub store: Arc<dyn BlobStore>,
    pub descriptors: DescriptorStore,
    pub fake: Arc<Fake>,
}

pub fn rig() -> Rig {
    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 28, 5));
    let fake = Arc::new(Fake::default());
    fake.nodes.store(3, Ordering::SeqCst);
    let names = || DescriptorStore::new(Arc::clone(&store), "", DeleteMode::Conditional);
    let cluster = Arc::new(ClusterService::new(Arc::clone(&store), "", 250).expect("cluster"));
    let cat = Arc::new(CatService::new(names().expect("names")));
    let health = Arc::new(HealthService::new(
        Arc::clone(&fake) as Arc<dyn HealthSource>,
        "test-cluster",
    ));
    let router = cluster_route::routes(Router::new(), cluster).expect("settings");
    let router = cat_route::routes(router, cat).expect("cat");
    let router = health_route::routes(router, health).expect("health");
    let server = Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    Rig {
        server,
        descriptors: names().expect("names"),
        store,
        fake,
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

pub fn get(r: &Rig, target: &str) -> support::Reply {
    call(r, "GET", target, None)
}

pub fn index(r: &Rig, name: &str, shards: u32) {
    r.descriptors
        .create(
            &IndexRecord::new(name, &format!("{name}-uuid"), shards),
            1,
            &mut vec![0u8; BUF],
        )
        .expect("created");
}

pub fn health_of(r: &Rig, index: &str, shards: &[(u32, HeadRead)]) {
    r.fake
        .indices
        .lock()
        .expect("lock")
        .insert(index.to_owned(), IndexInput::Shards(shards.to_vec()));
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

/// The status of `method target` whose body goes as `chunks` with no
/// declared length.
pub fn chunked_status(r: &Rig, method: &str, target: &str, chunks: &[&[u8]]) -> u16 {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(r.server.local_addr()).expect("connects");
    s.set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .expect("timeout");
    let head = format!(
        "{method} {target} HTTP/1.1\r\nhost: test\r\nconnection: close\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n"
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

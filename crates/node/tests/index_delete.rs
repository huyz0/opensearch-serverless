//! docs/specs/index-delete: `DELETE /{index}` with real heads and pins, and
//! the reclaim drain.

#[path = "support/mod.rs"]
mod support;

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oss_blob::{BlobStore, Emulator, SliceSource};
use oss_cluster::heads::shard_head_key;
use oss_format::{IndexRecord, PitShard, PointInTime, ShardHead};
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore, PitClock, PointInTimeStore};
use oss_node::index_delete::{DeleteService, NoSnapshots, routes};
use oss_rest::{Router, Server};

const BUF: usize = 1 << 16;
const NOW: u64 = 1_000_000;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
    service: Arc<DeleteService>,
    clock: Arc<AtomicU64>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let clock = Arc::new(AtomicU64::new(NOW));
    let tick = Arc::clone(&clock);
    let pit_clock: PitClock = Arc::new(move || tick.load(Ordering::Relaxed));
    let dynstore: Arc<dyn BlobStore> = store.clone();
    let service = Arc::new(
        DeleteService::new(
            &dynstore,
            "",
            DeleteMode::Conditional,
            pit_clock,
            Arc::new(NoSnapshots),
            BUF,
            2,
        )
        .expect("service"),
    );
    let router = routes(Router::new(), Arc::clone(&service)).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig {
        server,
        store,
        service,
        clock,
    }
}

fn delete(r: &Rig, target: &str) -> support::Reply {
    support::request(r.server.local_addr(), "DELETE", target, None)
}

fn create(r: &Rig, name: &str, uuid: &str, shards: u32) {
    let d = DescriptorStore::new(
        r.store.clone() as Arc<dyn BlobStore>,
        "",
        DeleteMode::Conditional,
    )
    .expect("store");
    d.create(
        &IndexRecord::new(name, uuid, shards),
        1,
        &mut vec![0u8; BUF],
    )
    .expect("created");
}

fn blob(r: &Rig, name: &str, uuid: &str, shard: u32) -> String {
    let key = format!("segments/{name}#{uuid}#{shard}/seg");
    r.store
        .put(&key, &mut SliceSource::new(b"bytes"), &mut [0u8; 16])
        .expect("put");
    key
}

fn present(r: &Rig, key: &str) -> bool {
    r.store.head(key).expect("head").is_some()
}

fn head(r: &Rig, name: &str, uuid: &str, shard: u32, owner: Option<&str>) -> String {
    let h = ShardHead {
        index: name.to_owned(),
        shard,
        term: 1,
        owner_node_id: owner.map(str::to_owned),
        owner_ephemeral_id: owner.map(str::to_owned),
        lease_expires_at_millis: 0,
        index_uuid: Some(uuid.to_owned()),
        writers: Vec::new(),
    };
    let key = shard_head_key("", name, shard);
    r.store
        .create_register(&key, &h.to_bytes().expect("bytes"))
        .expect("head");
    key
}

fn descriptor_gone(r: &Rig, name: &str) -> bool {
    let d = DescriptorStore::new(
        r.store.clone() as Arc<dyn BlobStore>,
        "",
        DeleteMode::Conditional,
    )
    .expect("store");
    d.get(name, &mut vec![0u8; BUF]).expect("get").is_none()
}

#[test]
fn a_delete_removes_the_descriptor_heads_and_unowned_bytes() {
    let r = rig();
    create(&r, "logs", "u1", 2);
    let (b0, b1) = (blob(&r, "logs", "u1", 0), blob(&r, "logs", "u1", 1));
    let h0 = head(&r, "logs", "u1", 0, None);
    let reply = delete(&r, "/logs");
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(reply.text(), r#"{"acknowledged":true,"index":"logs"}"#);
    assert!(descriptor_gone(&r, "logs"));
    assert!(!present(&r, &b0) && !present(&r, &b1));
    let mut buf = [0u8; 4096];
    assert!(
        r.store
            .read_register(&h0, &mut buf)
            .expect("read")
            .is_none()
    );
}

#[test]
fn a_shard_a_point_in_time_names_keeps_its_bytes() {
    let r = rig();
    create(&r, "logs", "u1", 2);
    let (b0, b1) = (blob(&r, "logs", "u1", 0), blob(&r, "logs", "u1", 1));
    let pits = PointInTimeStore::new(r.store.clone() as Arc<dyn BlobStore>, "", Arc::new(|| NOW))
        .expect("pits");
    let view = PointInTime {
        id: "AAAAAAAAAAAAAAAAAAAAAA".to_owned(),
        index: "logs".to_owned(),
        index_uuid: Some("u1".to_owned()),
        expires_at_millis: NOW + 600_000,
        capturing: false,
        shards: vec![PitShard {
            shard: 0,
            term: 1,
            files: Vec::new(),
        }],
    };
    pits.create(&view, &mut vec![0u8; BUF]).expect("pit");
    assert_eq!(delete(&r, "/logs").status, 200);
    assert!(present(&r, &b0), "pinned shard kept");
    assert!(!present(&r, &b1), "unpinned shard purged");
}

#[test]
fn an_owned_shard_waits_for_the_drain() {
    let r = rig();
    create(&r, "logs", "u1", 1);
    let b0 = blob(&r, "logs", "u1", 0);
    head(&r, "logs", "u1", 0, Some("n1"));
    assert_eq!(delete(&r, "/logs").status, 200);
    assert!(present(&r, &b0), "purge waits for the intent");
    assert_eq!(r.service.drain().expect("drain"), 0, "not due yet");
    r.clock.fetch_add(10 * 60_000, Ordering::Relaxed);
    assert_eq!(r.service.drain().expect("drain"), 1);
    assert!(!present(&r, &b0));
    assert_eq!(r.service.drain().expect("drain"), 0, "intent taken");
}

#[test]
fn a_missing_index_is_404_and_several_names_are_refused() {
    let r = rig();
    assert_eq!(delete(&r, "/nope").status, 404);
    assert_eq!(delete(&r, "/a,b").status, 400);
    assert_eq!(delete(&r, "/a*").status, 400);
}

#[test]
fn a_capture_in_progress_pins_every_shard() {
    let r = rig();
    create(&r, "logs", "u1", 2);
    let (b0, b1) = (blob(&r, "logs", "u1", 0), blob(&r, "logs", "u1", 1));
    let pits = PointInTimeStore::new(r.store.clone() as Arc<dyn BlobStore>, "", Arc::new(|| NOW))
        .expect("pits");
    let view = PointInTime {
        id: "BBBBBBBBBBBBBBBBBBBBBB".to_owned(),
        index: "logs".to_owned(),
        index_uuid: None,
        expires_at_millis: NOW + 600_000,
        capturing: true,
        shards: Vec::new(),
    };
    pits.create(&view, &mut vec![0u8; BUF]).expect("pit");
    assert_eq!(delete(&r, "/logs").status, 200);
    assert!(present(&r, &b0) && present(&r, &b1));
}

#[test]
fn a_view_of_another_index_pins_nothing() {
    let r = rig();
    create(&r, "logs", "u1", 1);
    let b0 = blob(&r, "logs", "u1", 0);
    let pits = PointInTimeStore::new(r.store.clone() as Arc<dyn BlobStore>, "", Arc::new(|| NOW))
        .expect("pits");
    let view = PointInTime {
        id: "CCCCCCCCCCCCCCCCCCCCCC".to_owned(),
        index: "other".to_owned(),
        index_uuid: Some("u9".to_owned()),
        expires_at_millis: NOW + 600_000,
        capturing: false,
        shards: vec![PitShard {
            shard: 0,
            term: 1,
            files: Vec::new(),
        }],
    };
    pits.create(&view, &mut vec![0u8; BUF]).expect("pit");
    assert_eq!(delete(&r, "/logs").status, 200);
    assert!(!present(&r, &b0));
}

#[test]
fn a_drain_with_nothing_due_is_one_list() {
    let r = rig();
    let before = r.store.meter().count(oss_blob::S3Op::List);
    assert_eq!(r.service.drain().expect("drain"), 0);
    assert_eq!(r.store.meter().count(oss_blob::S3Op::List) - before, 1);
}

#[test]
fn a_large_index_is_purged_at_the_reclaim_and_a_pinned_shard_when_its_view_ends() {
    let r = rig();
    create(&r, "big", "u1", 3);
    let blobs: Vec<String> = (0..3).map(|s| blob(&r, "big", "u1", s)).collect();
    assert_eq!(delete(&r, "/big").status, 200);
    assert!(
        blobs.iter().all(|k| present(&r, k)),
        "above the inline limit"
    );
    r.clock.fetch_add(10 * 60_000, Ordering::Relaxed);
    assert_eq!(r.service.drain().expect("drain"), 1);
    assert!(blobs.iter().all(|k| !present(&r, k)));

    // A view pins shard 0 of a small index: the drain keeps it, then lets go.
    create(&r, "small", "u2", 1);
    let kept = blob(&r, "small", "u2", 0);
    let now = r.clock.load(Ordering::Relaxed);
    let clock = Arc::clone(&r.clock);
    let pits = PointInTimeStore::new(
        r.store.clone() as Arc<dyn BlobStore>,
        "",
        Arc::new(move || clock.load(Ordering::Relaxed)),
    )
    .expect("pits");
    let view = PointInTime {
        id: "DDDDDDDDDDDDDDDDDDDDDD".to_owned(),
        index: "small".to_owned(),
        index_uuid: Some("u2".to_owned()),
        expires_at_millis: now + 20 * 60_000,
        capturing: false,
        shards: vec![PitShard {
            shard: 0,
            term: 1,
            files: Vec::new(),
        }],
    };
    pits.create(&view, &mut vec![0u8; BUF]).expect("pit");
    assert_eq!(delete(&r, "/small").status, 200);
    r.clock.fetch_add(10 * 60_000, Ordering::Relaxed);
    r.service.drain().expect("drain");
    assert!(present(&r, &kept), "still pinned");
    r.clock.fetch_add(30 * 60_000, Ordering::Relaxed);
    r.service.drain().expect("drain");
    assert!(!present(&r, &kept), "purged once the view expired");
}

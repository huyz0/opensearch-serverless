//! docs/specs/node-refresh-wait: a refresh waits for the search nodes
//! instead of publishing a commit, under the `serving` and `everywhere`
//! visibilities.

#![cfg(feature = "writer")]

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{NodeLogEpoch, ShardKey, WalRecord};
use oss_node::writer::{Publishing, RefreshVisibility, RoleTarget, Routes, TailSink, WriterRole};
use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, Owner, Planned, StreamConfig,
    stream,
};
use oss_writer::log::{
    CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, Refusal, ShardAdmission,
};
use oss_writer::mapping::Mapping;
use oss_writer::publish::lazy::Policy;

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

struct Local;

impl Routes for Local {
    fn index(&self, _: &str) -> Option<IndexInfo> {
        Some(info())
    }

    fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn forward(&self, _: &str, _: &IndexInfo, _: u32, _: &[Planned<'_>]) -> Vec<ItemResult> {
        Vec::new()
    }
}

fn info() -> IndexInfo {
    IndexInfo {
        name: "logs".into(),
        uuid: "logs-uuid".into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
    }
}

/// A sink that answers every wait with `confirms` and records what it was
/// asked.
struct Sink {
    confirms: bool,
    asked: Mutex<Vec<(String, u32, bool)>>,
}

impl Sink {
    fn new(confirms: bool) -> Arc<Self> {
        Arc::new(Self {
            confirms,
            asked: Mutex::new(Vec::new()),
        })
    }
}

impl TailSink for Sink {
    fn acked(&self, _: &str, _: &str, _: u32, _: &[WalRecord<'static>]) {}

    fn published(&self, _: &str, _: u32, _: i64) {}

    fn released(&self, _: &str, _: u32) {}

    fn await_visible(&self, uuid: &str, shard: u32, grants: bool) -> bool {
        self.asked
            .lock()
            .expect("lock")
            .push((uuid.to_owned(), shard, grants));
        self.confirms
    }
}

struct Rig {
    store: Arc<Emulator>,
    role: WriterRole,
}

/// A role under `visibility` with `sink` installed.
fn rig(visibility: RefreshVisibility, sink: Option<Arc<Sink>>) -> Rig {
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let log =
        NodeLogStore::new(Arc::clone(&store) as Arc<dyn BlobStore>, "", "node-1").expect("names");
    let committer = Arc::new(NodeLogCommitter::new(
        log,
        7,
        Box::new(Lease),
        Box::new(Admit),
        CommitterConfig {
            window: Duration::from_millis(0),
            ..CommitterConfig::default()
        },
    ));
    let ticking = Arc::new(AtomicU64::new(0));
    let publishing = Publishing::new(
        Arc::clone(&store) as Arc<dyn BlobStore>,
        "",
        "node-1",
        Policy::new(1_000_000, 0, 250),
        Box::new(move || ticking.load(Ordering::SeqCst)),
    );
    let role = WriterRole::new(committer, Arc::new(Local)).with_publishing(publishing);
    if let Some(sink) = sink {
        assert!(role.set_tail_sink(sink));
    }
    role.set_refresh_visibility(visibility);
    open(&role);
    Rig { store, role }
}

fn open(role: &WriterRole) {
    role.open_shard(
        &info(),
        0,
        3,
        Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping"),
    )
    .expect("opens");
}

fn write(role: &WriterRole) {
    let defaults = Defaults {
        index: Some("logs".into()),
        ..Defaults::default()
    };
    let budget = BulkBudget::new(1 << 20, 2);
    let config = StreamConfig {
        slice_bytes: 1 << 20,
        max_wait: Duration::from_secs(5),
    };
    let mut target = RoleTarget::new(role);
    let mut ids = IdGenerator::new(0, [0; 6]);
    let mut buffer = [0u8; 4096];
    let mut run = Bulk::new(&mut target, &mut ids, 1, false);
    let mut body: &[u8] = b"{\"index\":{\"_id\":\"a\"}}\n{\"n\":1}\n";
    stream(&mut body, &mut buffer, &defaults, &budget, config, &mut run).expect("streams");
    run.finish().expect("finishes");
}

fn puts(r: &Rig) -> u64 {
    r.store.meter().count(S3Op::Put)
}

fn found(r: &Rig) -> bool {
    r.role.get(&info(), 0, "a", false).expect("gets").is_some()
}

#[test]
fn a_confirmed_wait_makes_the_refresh_publish_nothing() {
    let sink = Sink::new(true);
    let r = rig(RefreshVisibility::Serving, Some(Arc::clone(&sink)));
    write(&r.role);
    let before = puts(&r);
    r.role.refresh(&info(), 0).expect("refreshes");
    assert_eq!(puts(&r), before, "no PUT");
    assert!(r.role.newest_commit("logs-uuid", 0).is_none());
    assert!(found(&r), "the engine refreshed");
    assert_eq!(
        *sink.asked.lock().expect("lock"),
        [("logs-uuid".to_owned(), 0, false)]
    );
}

#[test]
fn everywhere_asks_the_wait_to_cover_grants() {
    let sink = Sink::new(true);
    let r = rig(RefreshVisibility::Everywhere, Some(Arc::clone(&sink)));
    write(&r.role);
    r.role.refresh(&info(), 0).expect("refreshes");
    assert_eq!(
        *sink.asked.lock().expect("lock"),
        [("logs-uuid".to_owned(), 0, true)]
    );
}

#[test]
fn a_wait_that_does_not_confirm_falls_back_to_publishing() {
    let sink = Sink::new(false);
    let r = rig(RefreshVisibility::Serving, Some(Arc::clone(&sink)));
    write(&r.role);
    let before = puts(&r);
    r.role.refresh(&info(), 0).expect("refreshes");
    assert_eq!(puts(&r) - before, 2, "a blob and a swap");
    assert!(r.role.newest_commit("logs-uuid", 0).is_some());
    assert!(found(&r));
    assert_eq!(sink.asked.lock().expect("lock").len(), 1);
}

#[test]
fn the_owner_visibility_never_waits_and_publishes() {
    let sink = Sink::new(true);
    let r = rig(RefreshVisibility::Owner, Some(Arc::clone(&sink)));
    write(&r.role);
    r.role.refresh(&info(), 0).expect("refreshes");
    assert!(sink.asked.lock().expect("lock").is_empty());
    assert!(r.role.newest_commit("logs-uuid", 0).is_some());
}

#[test]
fn with_no_sink_a_waiting_visibility_publishes() {
    let r = rig(RefreshVisibility::Serving, None);
    write(&r.role);
    r.role.refresh(&info(), 0).expect("refreshes");
    assert!(r.role.newest_commit("logs-uuid", 0).is_some());
}

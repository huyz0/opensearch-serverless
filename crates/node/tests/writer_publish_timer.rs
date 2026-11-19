//! docs/specs/node-publish-timer: the thread that runs the publish pass.

#![cfg(feature = "writer")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{NodeLogEpoch, ShardKey};
use oss_node::writer::{PublishTimer, Publishing, RoleTarget, Routes, WriterRole};
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

struct Rig {
    store: Arc<Emulator>,
    role: Arc<WriterRole>,
}

/// A role publishing eagerly on the real clock.
fn rig() -> Rig {
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
    let publishing = Publishing::new(
        Arc::clone(&store) as Arc<dyn BlobStore>,
        "",
        "node-1",
        Policy::new(0, 0, 1),
        Box::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0))
        }),
    );
    let role = Arc::new(WriterRole::new(committer, Arc::new(Local)).with_publishing(publishing));
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
    let mut body: &[u8] = b"{\"index\":{}}\n{\"n\":1}\n";
    stream(&mut body, &mut buffer, &defaults, &budget, config, &mut run).expect("streams");
    run.finish().expect("finishes");
}

const TICK: Duration = Duration::from_millis(10);

fn puts(r: &Rig) -> u64 {
    r.store.meter().count(S3Op::Put)
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < until, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_shard_written_after_the_timer_started_is_published() {
    let r = rig();
    let _timer = PublishTimer::spawn(Arc::clone(&r.role), TICK);
    write(&r.role);
    wait_until("the shard's publish", || {
        r.role.newest_commit("logs-uuid", 0).is_some()
    });
}

#[test]
fn dropping_the_timer_stops_it_promptly_and_for_good() {
    let r = rig();
    let timer = PublishTimer::spawn(Arc::clone(&r.role), Duration::from_secs(30));
    let started = Instant::now();
    drop(timer);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "joined promptly"
    );
    write(&r.role);
    let before = puts(&r);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(puts(&r), before, "no pass after the drop");
    assert!(r.role.newest_commit("logs-uuid", 0).is_none());
}

#[test]
fn idle_ticks_make_no_request() {
    let r = rig();
    let before = puts(&r);
    let timer = PublishTimer::spawn(Arc::clone(&r.role), TICK);
    std::thread::sleep(Duration::from_millis(200));
    drop(timer);
    assert_eq!(puts(&r), before);
}

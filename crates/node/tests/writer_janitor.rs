//! docs/specs/node-janitor-pass: the janitor sweeps the shards a writer
//! holds, when they need it.

#![cfg(feature = "writer")]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::time::Instant;

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_format::{NodeLogEpoch, PitFile, PitShard, PointInTime, ShardKey};
use oss_janitor::gc::Config;
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_meta::PointInTimeStore;
use oss_node::writer::{
    JanitorPass, JanitorReport, JanitorTimer, PassLimits, Publishing, RoleTarget, Routes,
    WriterRole,
};
use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, Owner, Planned, StreamConfig,
    stream,
};
use oss_writer::log::{
    CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, Refusal, ShardAdmission,
};
use oss_writer::mapping::Mapping;
use oss_writer::publish::lazy::Policy;

const PATH: &str = "segments/logs#logs-uuid#0/";
const ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";
const BUFFER: usize = 1 << 16;

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

const LIVE: Config = Config {
    dry_run: false,
    minimum_unreferenced_millis: 0,
    max_watched: 1000,
    max_pinned: 1000,
};

struct Rig {
    store: Arc<Emulator>,
    role: Arc<WriterRole>,
    pass: JanitorPass,
    pits: PointInTimeStore,
}

fn rig_in(pool_bytes: usize, buffer_bytes: usize) -> Rig {
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let dynamic = Arc::clone(&store) as Arc<dyn BlobStore>;
    let log = NodeLogStore::new(Arc::clone(&dynamic), "", "node-1").expect("names");
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
        Arc::clone(&dynamic),
        "",
        "node-1",
        Policy::new(0, 0, 250),
        Box::new(|| 0),
    );
    let role = Arc::new(WriterRole::new(committer, Arc::new(Local)).with_publishing(publishing));
    role.open_shard(
        &info(),
        0,
        3,
        Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping"),
    )
    .expect("opens");
    let clock = Arc::new(AtomicU64::new(1_000));
    let read = Arc::clone(&clock);
    let pits = PointInTimeStore::new(
        Arc::clone(&dynamic),
        "",
        Arc::new(move || read.load(Ordering::SeqCst)),
    )
    .expect("pits");
    let budget = MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::RequestBodies, pool_bytes)
            .expect("fits"),
    );
    let read = Arc::clone(&clock);
    let pass = JanitorPass::new(
        dynamic,
        "",
        Arc::clone(&role),
        PointInTimeStore::new(
            Arc::clone(&store) as Arc<dyn BlobStore>,
            "",
            Arc::new(move || read.load(Ordering::SeqCst)),
        )
        .expect("pits"),
        &budget,
        PassLimits {
            buffer_bytes,
            max_views: 100,
        },
        Box::new(|| 5_000),
    );
    Rig {
        store,
        role,
        pass,
        pits,
    }
}

fn rig() -> Rig {
    rig_in(1 << 20, BUFFER)
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

/// A written, published shard and an older term's blob nothing names.
fn published_with_an_orphan(r: &Rig) -> String {
    write(&r.role);
    assert_eq!(r.role.publish_due().published, 1);
    let key = format!("{PATH}t=1/orphan");
    r.store
        .put(&key, &mut SliceSource::new(b"old"), &mut [0u8; 16])
        .expect("planted");
    key
}

fn requests(r: &Rig) -> u64 {
    S3Op::ALL.iter().map(|op| r.store.meter().count(*op)).sum()
}

fn exists(r: &Rig, key: &str) -> bool {
    r.store.head(key).expect("heads").is_some()
}

#[test]
fn a_held_shard_that_published_nothing_makes_no_request() {
    let r = rig();
    let before = requests(&r);
    assert_eq!(r.pass.run(LIVE), JanitorReport::default());
    assert_eq!(requests(&r), before);
}

#[test]
fn an_unreferenced_blob_goes_on_the_second_sighting_and_then_the_shard_rests() {
    let r = rig();
    let key = published_with_an_orphan(&r);
    let first = r.pass.run(LIVE);
    assert_eq!((first.swept, first.deleted), (1, 0), "{first:?}");
    assert!(exists(&r, &key), "seen once is not enough");
    // Nothing published since, but a blob is being watched.
    let second = r.pass.run(LIVE);
    assert_eq!((second.swept, second.deleted), (1, 1), "{second:?}");
    assert!(!exists(&r, &key));
    let before = requests(&r);
    assert_eq!(r.pass.run(LIVE), JanitorReport::default());
    assert_eq!(requests(&r), before, "a rested shard costs nothing");
}

#[test]
fn a_dry_run_counts_and_deletes_nothing() {
    let r = rig();
    let key = published_with_an_orphan(&r);
    let dry = Config {
        dry_run: true,
        ..LIVE
    };
    r.pass.run(dry);
    let second = r.pass.run(dry);
    assert_eq!((second.deleted, second.would_delete), (0, 1), "{second:?}");
    assert!(exists(&r, &key));
}

#[test]
fn a_point_in_time_keeps_the_blob_it_names() {
    let r = rig();
    let key = published_with_an_orphan(&r);
    let view = PointInTime {
        id: ID.to_owned(),
        index: "logs".to_owned(),
        index_uuid: Some("logs-uuid".to_owned()),
        expires_at_millis: 9_000_000,
        capturing: false,
        shards: vec![PitShard {
            shard: 0,
            term: 1,
            files: vec![PitFile {
                name: "orphan".to_owned(),
                location: "t=1".to_owned(),
                length: Some(3),
            }],
        }],
    };
    r.pits.create(&view, &mut vec![0u8; BUFFER]).expect("view");
    r.pass.run(LIVE);
    let second = r.pass.run(LIVE);
    assert_eq!(second.deleted, 0, "{second:?}");
    assert!(exists(&r, &key));
}

#[test]
fn a_new_commit_makes_the_shard_due_again() {
    let r = rig();
    published_with_an_orphan(&r);
    r.pass.run(LIVE);
    r.pass.run(LIVE);
    let rested = requests(&r);
    write(&r.role);
    assert_eq!(r.role.publish_due().published, 1);
    let after = requests(&r);
    let pass = r.pass.run(LIVE);
    assert_eq!(pass.swept, 1, "{pass:?}");
    assert!(requests(&r) > after, "a swept shard is read");
    assert!(after > rested);
}

#[test]
fn a_pass_with_no_memory_for_its_buffer_sweeps_nothing() {
    let r = rig_in(BUFFER - 1, BUFFER);
    published_with_an_orphan(&r);
    let pass = r.pass.run(LIVE);
    assert_eq!(pass.swept, 0);
    assert!(pass.skipped.is_some(), "{pass:?}");
}

#[test]
fn a_shard_pinned_whole_is_swept_again_once_the_pin_is_gone() {
    let r = rig();
    let key = published_with_an_orphan(&r);
    let capture = PointInTime {
        id: ID.to_owned(),
        index: "logs".to_owned(),
        index_uuid: None,
        expires_at_millis: 9_000_000,
        capturing: true,
        shards: Vec::new(),
    };
    r.pits
        .create(&capture, &mut vec![0u8; BUFFER])
        .expect("capture");
    let held = r.pass.run(LIVE);
    assert_eq!(
        (held.swept, held.pinned, held.deleted),
        (1, 1, 0),
        "{held:?}"
    );
    assert!(
        r.pits
            .release(ID, &mut vec![0u8; BUFFER])
            .expect("released")
    );
    // Nothing published since, and nothing watched, yet the shard is due.
    let first = r.pass.run(LIVE);
    assert_eq!((first.swept, first.pinned), (1, 0), "{first:?}");
    let second = r.pass.run(LIVE);
    assert_eq!(second.deleted, 1, "{second:?}");
    assert!(!exists(&r, &key));
}

#[test]
fn the_timer_runs_a_pass_on_its_interval_reading_the_config_each_time_until_dropped() {
    let r = rig();
    let reads = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&reads);
    let timer = JanitorTimer::spawn(
        Arc::new(r.pass),
        Duration::from_millis(5),
        Box::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            LIVE
        }),
    );
    let started = Instant::now();
    while reads.load(Ordering::SeqCst) < 3 {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "too few passes"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    drop(timer);
    let stopped = reads.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(
        reads.load(Ordering::SeqCst),
        stopped,
        "a dropped timer runs no pass"
    );
}

#[test]
fn the_timer_waits_its_interval_before_the_first_pass_and_stops_promptly() {
    let r = rig();
    let reads = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&reads);
    let timer = JanitorTimer::spawn(
        Arc::new(r.pass),
        Duration::from_secs(30),
        Box::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            LIVE
        }),
    );
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "no pass before the interval"
    );
    let started = Instant::now();
    drop(timer);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a sleeping timer is woken to stop"
    );
}

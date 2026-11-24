//! docs/specs/node-publish-loop: the writer role publishes the shards it
//! holds on the lazy policy.

#![cfg(feature = "writer")]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{CommitManifest, NodeLogEpoch, ShardKey, ShardRef};
use oss_node::writer::{Publishing, RoleTarget, Routes, WriterRole};
use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, Owner, Planned, StreamConfig,
    stream,
};
use oss_writer::log::{
    CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, Refusal, ShardAdmission,
};
use oss_writer::mapping::Mapping;
use oss_writer::publish::ShardPublisher;
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
        default_pipeline: None,
        final_pipeline: None,
    }
}

struct Rig {
    store: Arc<Emulator>,
    role: WriterRole,
    clock: Arc<AtomicU64>,
}

/// A role publishing under `max_age` with a 250 ms tick.
fn rig(max_age: u64) -> Rig {
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
    let clock = Arc::new(AtomicU64::new(0));
    let ticking = Arc::clone(&clock);
    let publishing = Publishing::new(
        Arc::clone(&store) as Arc<dyn BlobStore>,
        "",
        "node-1",
        Policy::new(max_age, 0, 250),
        Box::new(move || ticking.load(Ordering::SeqCst)),
    );
    let role = WriterRole::new(committer, Arc::new(Local)).with_publishing(publishing);
    open(&role);
    Rig { store, role, clock }
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

fn at(r: &Rig, now: u64) {
    r.clock.store(now, Ordering::SeqCst);
}

fn puts(r: &Rig) -> u64 {
    r.store.meter().count(S3Op::Put)
}

fn published(r: &Rig) -> usize {
    r.role.publish_due().published
}

#[test]
fn the_first_pass_publishes_a_written_shard_for_a_blob_and_a_swap() {
    let r = rig(1_000);
    write(&r.role);
    let before = (puts(&r), r.store.meter().count(S3Op::List));
    assert_eq!(published(&r), 1);
    assert_eq!(puts(&r) - before.0, 2, "a blob and a swap");
    assert_eq!(r.store.meter().count(S3Op::List), before.1, "no LIST");
    let newest = r.role.newest_commit("logs-uuid", 0).expect("staged");
    let manifest = CommitManifest::parse(&newest).expect("parses");
    assert_eq!(manifest.term, 3);
    assert_eq!(manifest.writer.as_deref(), Some("node-1"));
    assert_eq!(manifest.wal_ordinal, -1, "the log ordinal is not known");
}

#[test]
fn a_shard_with_no_write_publishes_nothing() {
    let r = rig(1_000);
    let before = puts(&r);
    assert_eq!(published(&r), 0);
    assert_eq!(puts(&r), before);
    assert!(r.role.newest_commit("logs-uuid", 0).is_none());
}

#[test]
fn a_later_write_waits_for_its_age_rounded_up_to_the_tick() {
    let r = rig(1_000);
    write(&r.role);
    assert_eq!(published(&r), 1);
    at(&r, 100);
    write(&r.role);
    // Due at ceil((100 + 1000) / 250) * 250 = 1250.
    at(&r, 1_249);
    assert_eq!(published(&r), 0);
    at(&r, 1_250);
    assert_eq!(published(&r), 1);
    at(&r, 5_000);
    assert_eq!(published(&r), 0, "nothing written since");
}

// docs/specs/tenant-publish-stretch AC-3
#[test]
fn a_stretch_asked_by_index_name_lengthens_the_wait_of_that_index() {
    let r = rig(1_000);
    assert!(
        r.role
            .set_publish_stretch(Arc::new(|index| if index == "logs" { 4 } else { 1 }))
    );
    assert!(!r.role.set_publish_stretch(Arc::new(|_| 1)), "once");
    write(&r.role);
    assert_eq!(published(&r), 1);
    at(&r, 100);
    write(&r.role);
    // Due at ceil((100 + 4 * 1000) / 250) * 250 = 4250.
    at(&r, 4_249);
    assert_eq!(published(&r), 0);
    at(&r, 4_250);
    assert_eq!(published(&r), 1);
}

#[test]
fn age_zero_publishes_every_pass_with_a_write() {
    let r = rig(0);
    write(&r.role);
    assert_eq!(published(&r), 1);
    assert_eq!(published(&r), 0, "nothing written since");
    write(&r.role);
    assert_eq!(published(&r), 1);
}

#[test]
fn a_refused_publish_is_reported_and_tried_again() {
    let r = rig(0);
    write(&r.role);
    // A newer term holds the shard's register.
    let shard = ShardRef {
        index: "logs",
        uuid: "logs-uuid",
        shard: 0,
    };
    let mut other = ShardPublisher::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "", &shard)
        .expect("publisher");
    let dir = lucene_store::ByteBuffersDirectory::new();
    let _ = other.publish(&dir, &[], 9, "node-2", -1, 0);
    let pass = r.role.publish_due();
    assert_eq!(pass.published, 0);
    assert_eq!(pass.failed.len(), 1, "{:?}", pass.failed);
    assert!(r.role.newest_commit("logs-uuid", 0).is_none());
    assert_eq!(r.role.publish_due().failed.len(), 1, "tried again");
}

#[test]
fn a_reopened_shard_publishes_at_its_first_write() {
    let r = rig(1_000);
    write(&r.role);
    assert_eq!(published(&r), 1);
    open(&r.role);
    assert_eq!(published(&r), 0, "no write since it opened");
    write(&r.role);
    assert_eq!(published(&r), 1, "never published as far as it knows");
}

#[test]
fn a_refresh_publishes_what_was_written_and_the_timer_then_finds_nothing() {
    let r = rig(1_000);
    write(&r.role);
    r.role.refresh(&info(), 0).expect("refreshes");
    let newest = r.role.newest_commit("logs-uuid", 0).expect("staged");
    assert_eq!(CommitManifest::parse(&newest).expect("parses").term, 3);
    assert_eq!(published(&r), 0, "the refresh published it");
}

#[test]
fn a_refresh_of_a_shard_not_held_is_refused() {
    let r = rig(1_000);
    let mut other = info();
    other.uuid = "other".into();
    assert!(r.role.refresh(&other, 0).is_err());
}

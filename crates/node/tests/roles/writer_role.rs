//! The node writer role on the emulator: a write is applied, staged in the
//! node's one log, and only then acknowledged.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{NodeLogEntry, NodeLogEpoch, ShardKey};
use oss_node::writer::{RoleTarget, Routes, WriterRole};
use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, Item, ItemResult, Owner, Planned,
    StreamConfig, stream,
};
use oss_writer::log::{
    CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, Refusal, ShardAdmission,
};
use oss_writer::mapping::Mapping;

#[derive(Default)]
struct Lease(Mutex<Vec<Vec<NodeLogEpoch>>>);

impl EpochLease for Lease {
    fn record(&self, epochs: &[NodeLogEpoch]) -> Result<(), Refusal> {
        self.0.lock().expect("lock").push(epochs.to_vec());
        Ok(())
    }
}

#[derive(Default)]
struct Admission {
    refuse: Mutex<BTreeSet<String>>,
    refuse_first: Mutex<u32>,
}

impl ShardAdmission for Admission {
    fn admit(&self, shard: &ShardKey, _: i64, _: u64) -> Result<(), Refusal> {
        let mut first = self.refuse_first.lock().expect("lock");
        if *first > 0 {
            *first = first.saturating_sub(1);
            return Err(Arc::new(std::io::Error::other("not the owner")));
        }
        if self
            .refuse
            .lock()
            .expect("lock")
            .contains(shard.index_uuid.as_ref())
        {
            return Err(Arc::new(std::io::Error::other("not the owner")));
        }
        Ok(())
    }
}

#[derive(Default)]
struct Cluster {
    forwarded: Mutex<Vec<(String, u32, usize)>>,
    owner: Mutex<Option<String>>,
}

impl Routes for Cluster {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        Some(info(name, 1))
    }

    fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
        match self.owner.lock().expect("lock").clone() {
            Some(node) => Owner::Node(node),
            None => Owner::Local,
        }
    }

    fn forward(
        &self,
        node: &str,
        _: &IndexInfo,
        shard: u32,
        group: &[Planned<'_>],
    ) -> Vec<ItemResult> {
        self.forwarded
            .lock()
            .expect("lock")
            .push((node.to_owned(), shard, group.len()));
        group
            .iter()
            .map(|_| ItemResult::Written {
                written: oss_writer::index::Written {
                    outcome: oss_writer::index::Outcome::Created,
                    version: 1,
                    seq_no: 0,
                    primary_term: 1,
                },
                forced_refresh: false,
            })
            .collect()
    }
}

fn info(name: &str, shards: u32) -> IndexInfo {
    IndexInfo {
        name: name.into(),
        uuid: format!("{name}-uuid"),
        shards,
        partition_size: 1,
        routing_shards: shards,
        routing_required: false,
    }
}

pub(super) struct Rig {
    store: Arc<Emulator>,
    log: NodeLogStore,
    lease: Arc<Lease>,
    admission: Arc<Admission>,
    cluster: Arc<Cluster>,
    pub(super) role: WriterRole,
}

pub(super) fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let log =
        NodeLogStore::new(Arc::clone(&store) as Arc<dyn BlobStore>, "", "node-1").expect("names");
    let lease = Arc::new(Lease::default());
    let admission = Arc::new(Admission::default());
    let cluster = Arc::new(Cluster::default());
    let committer = Arc::new(NodeLogCommitter::new(
        log.clone(),
        7,
        Box::new(Arc::clone(&lease)),
        Box::new(Arc::clone(&admission)),
        CommitterConfig {
            window: Duration::from_millis(0),
            ..CommitterConfig::default()
        },
    ));
    let role = WriterRole::new(committer, Arc::clone(&cluster) as Arc<dyn Routes>);
    Rig {
        store,
        log,
        lease,
        admission,
        cluster,
        role,
    }
}

pub(super) fn open(r: &Rig, name: &str, shards: u32) {
    let i = info(name, shards);
    for s in 0..shards {
        r.role
            .open_shard(
                &i,
                s,
                3,
                Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping"),
            )
            .expect("opens");
    }
}

pub(super) fn bulk(r: &Rig, index: &str, body: &str) -> Vec<Item> {
    let defaults = Defaults {
        index: Some(index.into()),
        ..Defaults::default()
    };
    let budget = BulkBudget::new(1 << 20, 2);
    let config = StreamConfig {
        slice_bytes: 1 << 20,
        max_wait: Duration::from_secs(5),
    };
    let mut target = RoleTarget::new(&r.role);
    let mut ids = IdGenerator::new(0, [0; 6]);
    let mut buffer = [0u8; 4096];
    let mut run = Bulk::new(&mut target, &mut ids, 1, false);
    stream(
        &mut body.as_bytes(),
        &mut buffer,
        &defaults,
        &budget,
        config,
        &mut run,
    )
    .expect("streams");
    run.finish().expect("finishes")
}

pub(super) fn docs(ids: impl Iterator<Item = String>) -> String {
    ids.map(|id| format!("{{\"index\":{{\"_id\":\"{id}\"}}}}\n{{\"n\":1}}\n"))
        .collect()
}

pub(super) fn puts(r: &Rig) -> u64 {
    r.store.meter().count(S3Op::Put)
}

pub(super) fn written(items: &[Item]) -> usize {
    items
        .iter()
        .filter(|i| matches!(i.result, ItemResult::Written { .. }))
        .count()
}

fn blob_shards(r: &Rig, ordinal: u64) -> Vec<(String, u32)> {
    match r.log.read_entry(7, ordinal).expect("reads") {
        Some(NodeLogEntry::Records(h)) => h
            .index
            .iter()
            .map(|(k, _)| (k.index_uuid.to_string(), k.shard))
            .collect(),
        other => panic!("expected records, got {other:?}"),
    }
}

#[test]
fn a_write_returns_after_one_blob_holding_its_record() {
    let r = rig();
    open(&r, "logs", 1);
    let items = bulk(&r, "logs", &docs(["a".to_owned()].into_iter()));
    assert_eq!(written(&items), 1);
    assert_eq!(puts(&r), 1, "one blob");
    assert_eq!(blob_shards(&r, 0), [("logs-uuid".to_owned(), 0)]);
    let ItemResult::Written { written: w, .. } = &items[0].result else {
        panic!("written");
    };
    assert_eq!((w.seq_no, w.primary_term, w.version), (0, 3, 1));
}

#[test]
fn the_first_write_records_one_epoch_and_later_ones_none() {
    let r = rig();
    open(&r, "logs", 1);
    bulk(&r, "logs", &docs(["a".to_owned()].into_iter()));
    bulk(&r, "logs", &docs(["b".to_owned()].into_iter()));
    let writes = r.lease.0.lock().expect("lock");
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].len(), 1);
    assert_eq!(writes[0][0].epoch, 7);
    assert_eq!(puts(&r), 2, "the lease here is not a blob of the store");
}

#[test]
fn a_group_of_fifty_is_one_blob_and_two_shards_are_one_blob() {
    let r = rig();
    open(&r, "one", 1);
    let items = bulk(&r, "one", &docs((0..50).map(|i| i.to_string())));
    assert_eq!(written(&items), 50);
    assert_eq!(puts(&r), 1, "fifty items, one record group, one blob");

    open(&r, "wide", 4);
    let before = puts(&r);
    let items = bulk(&r, "wide", &docs((0..64).map(|i| i.to_string())));
    assert_eq!(written(&items), 64);
    assert_eq!(puts(&r) - before, 1, "four shards in one request, one blob");
    assert!(blob_shards(&r, 1).len() > 1);
}

#[test]
fn a_refused_write_logs_nothing_and_takes_no_sequence_number() {
    let r = rig();
    open(&r, "logs", 1);
    bulk(&r, "logs", &docs(["a".to_owned()].into_iter()));
    let before = puts(&r);
    let create = "{\"create\":{\"_id\":\"a\"}}\n{\"n\":1}\n";
    let items = bulk(&r, "logs", create);
    assert!(matches!(
        items[0].result,
        ItemResult::Failed { status: 409, .. }
    ));
    assert_eq!(puts(&r), before, "nothing logged");
    let items = bulk(&r, "logs", &docs(["b".to_owned()].into_iter()));
    let ItemResult::Written { written: w, .. } = &items[0].result else {
        panic!("written");
    };
    assert_eq!(w.seq_no, 1, "the conflict took none");
}

#[test]
fn a_failed_stage_refuses_the_write_and_drops_the_shard() {
    let r = rig();
    open(&r, "logs", 1);
    r.admission
        .refuse
        .lock()
        .expect("lock")
        .insert("logs-uuid".into());
    let items = bulk(&r, "logs", &docs(["a".to_owned()].into_iter()));
    assert_eq!(written(&items), 0);
    assert!(matches!(items[0].result, ItemResult::Failed { .. }));
    assert!(!r.role.held("logs-uuid", 0), "the slot is released");
    r.admission.refuse.lock().expect("lock").clear();
    let items = bulk(&r, "logs", &docs(["a".to_owned()].into_iter()));
    assert_eq!(written(&items), 0, "not held, so refused again");
}

#[test]
fn a_foreign_shard_is_forwarded_once_and_logs_nothing_here() {
    let r = rig();
    *r.cluster.owner.lock().expect("lock") = Some("n2".into());
    let items = bulk(&r, "logs", &docs((0..3).map(|i| i.to_string())));
    assert_eq!(written(&items), 3);
    assert_eq!(
        *r.cluster.forwarded.lock().expect("lock"),
        [("n2".to_owned(), 0, 3)]
    );
    assert_eq!(puts(&r), 0);
}

#[test]
fn a_shard_nobody_holds_fails_and_logs_nothing() {
    let r = rig();
    let items = bulk(&r, "logs", &docs(["a".to_owned()].into_iter()));
    assert!(matches!(items[0].result, ItemResult::Failed { .. }));
    assert_eq!(puts(&r), 0);
}

#[test]
fn deletes_and_routed_writes_are_staged_and_a_refresh_reaches_the_engine() {
    let r = rig();
    open(&r, "logs", 1);
    bulk(&r, "logs", &docs(["a".to_owned()].into_iter()));
    let before = puts(&r);
    let body =
        "{\"delete\":{\"_id\":\"a\"}}\n{\"index\":{\"_id\":\"b\",\"routing\":\"x\"}}\n{\"n\":1}\n";
    let items = bulk(&r, "logs", body);
    assert_eq!(written(&items), 2);
    assert_eq!(puts(&r) - before, 1, "both in one blob");
    let i = info("logs", 1);
    r.role.refresh(&i, 0).expect("refreshes");
    assert!(r.role.refresh(&info("nope", 1), 0).is_err());
}

#[test]
fn one_item_applied_alone_is_staged_too() {
    use oss_writer::bulk::{Kind, Op, Target};
    let r = rig();
    open(&r, "logs", 1);
    let i = info("logs", 1);
    let defaults = Defaults::default();
    let mut reader = oss_writer::bulk::BodyReader::new(&defaults);
    reader
        .feed(b"{\"index\":{\"_index\":\"logs\",\"_id\":\"a\"}}\n{\"n\":1}\n")
        .expect("parses");
    reader.finish().expect("ends");
    let ops = reader.take();
    let op: &Op = &ops[0];
    assert_eq!(op.action.kind, Kind::Index);
    let planned = Planned {
        position: 0,
        op,
        id: "a",
    };
    let mut target = RoleTarget::new(&r.role);
    assert!(target.apply(&i, 0, &planned).is_ok());
    assert_eq!(puts(&r), 1);
    assert!(target.apply(&info("other", 1), 0, &planned).is_err());
}

#[test]
fn a_write_waiting_on_a_shard_whose_stage_failed_is_refused_too() {
    let r = Arc::new(rig());
    open(&r, "logs", 1);
    // Only the first stage is refused; a writer that waited on the slot
    // must not land on the engine that holds the refused write.
    *r.admission.refuse_first.lock().expect("lock") = 1;
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let r = Arc::clone(&r);
            std::thread::spawn(move || {
                written(&bulk(&r, "logs", &docs(std::iter::once(format!("d{t}")))))
            })
        })
        .collect();
    let acked: usize = threads.into_iter().map(|t| t.join().expect("ends")).sum();
    assert_eq!(acked, 0, "every writer saw the failed shard");
    assert!(!r.role.held("logs-uuid", 0));
}

fn limited(max: usize) -> (Rig, oss_writer::admission::WriteLimiter) {
    let r = rig();
    let limiter = oss_writer::admission::LimiterConfig {
        min_in_flight: 1,
        max_in_flight: max,
        ..Default::default()
    }
    .build();
    assert!(r.role.set_write_limiter(limiter.clone()));
    assert!(!r.role.set_write_limiter(limiter.clone()), "one limiter");
    (r, limiter)
}

// docs/specs/write-backpressure AC-6
#[test]
fn a_group_the_limiter_refuses_is_neither_applied_nor_logged_and_answers_429() {
    let (r, limiter) = limited(1);
    open(&r, "logs", 1);
    // Another write holds the one slot.
    let held = limiter.admit(0).expect("the slot");
    let before = puts(&r);
    let items = bulk(
        &r,
        "logs",
        &docs(["a".to_owned(), "b".to_owned()].into_iter()),
    );
    assert_eq!(items.len(), 2);
    for item in &items {
        let ItemResult::Failed { status, error } = &item.result else {
            panic!("refused");
        };
        assert_eq!(*status, 429);
        assert!(
            error.reason().contains("slow to take writes"),
            "{}",
            error.reason()
        );
    }
    assert_eq!(puts(&r), before, "nothing logged");
    assert_eq!(limiter.stats().refused, 1, "one refusal, for the group");
    drop(held);
    let items = bulk(&r, "logs", &docs(["c".to_owned()].into_iter()));
    let ItemResult::Written { written: w, .. } = &items[0].result else {
        panic!("admitted once the slot is free");
    };
    assert_eq!(w.seq_no, 0, "the refused group took no sequence number");
    assert_eq!(limiter.stats().in_flight, 0, "the slot is given back");
}

// docs/specs/write-backpressure AC-2
#[test]
fn a_failed_log_append_lowers_the_limit_and_a_landed_one_does_not() {
    let (r, limiter) = limited(100);
    open(&r, "logs", 1);
    let items = bulk(&r, "logs", &docs(["a".to_owned()].into_iter()));
    assert_eq!(written(&items), 1);
    assert_eq!(limiter.stats().limit, 100, "a fast append never cuts");

    // The first write of a shard records its epoch, which is where the
    // refusal is raised.
    let (r, limiter) = limited(100);
    open(&r, "logs", 1);
    r.admission
        .refuse
        .lock()
        .expect("lock")
        .insert("logs-uuid".into());
    let items = bulk(&r, "logs", &docs(["b".to_owned()].into_iter()));
    assert_eq!(written(&items), 0);
    assert_eq!(limiter.stats().limit, 66, "100 cut by a third");
}

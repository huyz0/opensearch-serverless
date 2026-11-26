//! docs/specs/node-takeover: a node opens a shard a previous writer left a
//! commit and an unpublished log for.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_cluster::heads::ShardHeadStore;
use oss_cluster::{LeaseClock, LeaseConfig, LivenessOracle, Membership, SystemClock};
use oss_format::{CommitManifest, IndexRecord, NodeLease, ShardRef};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::descriptor_routes::DescriptorRoutes;
use oss_node::writer::{
    DescriptorIncarnation, HeadAdmission, HeadOpener, MembershipLeaseWrite, Publishing, RoleTarget,
    ShardNames, Takeover, WriterRole,
};
use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, StreamConfig, stream,
};
use oss_writer::log::{
    CommitterConfig, LeaseKeeper, NodeLogCommitter, NodeLogStore, NodeLogTakeover,
};
use oss_writer::publish::lazy::Policy;
use oss_writer::takeover::{HeadWriters, Predecessors};

const MAPPING: &str = r#"{"properties":{"n":{"type":"integer"}}}"#;

/// The clock an hour on: every lease node-1 holds has lapsed by it.
struct Later;

impl LeaseClock for Later {
    fn now_millis(&self) -> i64 {
        SystemClock.now_millis().saturating_add(3_600_000)
    }

    fn pause(&self, pause: Duration) {
        SystemClock.pause(pause);
    }
}

pub(crate) struct Cluster {
    pub(crate) store: Arc<Emulator>,
    pub(crate) descriptors: Arc<DescriptorStore>,
}

pub(crate) struct Node {
    pub(crate) role: Arc<WriterRole>,
    pub(crate) committer: Arc<NodeLogCommitter>,
    pub(crate) log: NodeLogStore,
}

pub(crate) fn cluster() -> Cluster {
    let store = Arc::new(Emulator::new(64 << 20, 1));
    let dynstore: Arc<dyn BlobStore> = store.clone();
    let membership = Arc::new(
        Membership::new(
            Arc::clone(&dynstore),
            "",
            Arc::new(SystemClock),
            LeaseConfig {
                ttl_millis: 30_000,
                max_members: 16,
                conditional_deletes: false,
            },
        )
        .expect("membership"),
    );
    for (node, eph) in [("node-1", "e1"), ("node-2", "e2")] {
        let mut lease = NodeLease::new(node, eph, "a", 0);
        lease.set_roles(["ingest".to_owned()]);
        membership.renew(&lease).expect("enrolled");
    }
    let descriptors = Arc::new(
        DescriptorStore::new(Arc::clone(&dynstore), "", DeleteMode::Tombstone)
            .expect("descriptors"),
    );
    Cluster { store, descriptors }
}

/// A node `id`; one that is `later` judges leases and head claims an hour on,
/// so it sees every other node as dead.
fn node(c: &Cluster, id: &str, term: u64, later: bool, takeover: Option<Takeover>) -> Node {
    node_with(c, id, term, later, takeover, None)
}

/// [`node`], staging its routine commits when `staging` gives a checkpoint age.
pub(crate) fn node_with(
    c: &Cluster,
    id: &str,
    term: u64,
    later: bool,
    takeover: Option<Takeover>,
    staging: Option<Duration>,
) -> Node {
    let clock: Arc<dyn LeaseClock> = if later {
        Arc::new(Later)
    } else {
        Arc::new(SystemClock)
    };
    let membership = Arc::new(
        Membership::new(
            c.store.clone(),
            "",
            Arc::clone(&clock),
            LeaseConfig {
                ttl_millis: 30_000,
                max_members: 16,
                conditional_deletes: false,
            },
        )
        .expect("membership"),
    );
    let dynstore: Arc<dyn BlobStore> = c.store.clone();
    let eph = format!("e{}", &id[5..]);
    let heads = Arc::new(
        ShardHeadStore::new(
            Arc::clone(&dynstore),
            "",
            clock,
            30_000,
            Arc::new(LivenessOracle::new(Arc::clone(&membership))),
        )
        .expect("heads"),
    );
    let log = NodeLogStore::new(Arc::clone(&dynstore), "", id).expect("names");
    let names = Arc::new(ShardNames::default());
    let mut lease = NodeLease::new(id, &eph, "a1", 0);
    lease.set_roles(["ingest".to_owned()]);
    let keeper = Arc::new(LeaseKeeper::new(
        Box::new(MembershipLeaseWrite::new(membership, lease)),
        log.clone(),
        Vec::new(),
    ));
    let committer = Arc::new(NodeLogCommitter::new(
        log.clone(),
        term,
        Box::new(Arc::clone(&keeper)),
        Box::new(HeadAdmission::new(
            Arc::clone(&heads),
            Arc::clone(&names),
            id,
            &eph,
        )),
        CommitterConfig {
            window: Duration::ZERO,
            ..CommitterConfig::default()
        },
    ));
    let routes = Arc::new(DescriptorRoutes::new(
        DescriptorStore::new(Arc::clone(&dynstore), "", DeleteMode::Tombstone).expect("names"),
        1 << 16,
    ));
    let mut opener = HeadOpener::new(
        heads,
        Arc::clone(&c.descriptors),
        Arc::clone(&dynstore),
        "",
        id,
        &eph,
        1 << 16,
    );
    if let Some(takeover) = takeover {
        opener = opener.with_takeover(takeover);
    }
    let mut publishing =
        Publishing::new(dynstore, "", id, Policy::new(0, 0, 250), Box::new(|| 1_000));
    if let Some(age) = staging {
        publishing = publishing.with_staging(age);
    }
    let role = WriterRole::new(Arc::clone(&committer), routes)
        .with_names(names)
        .with_incarnation(
            Arc::new(DescriptorIncarnation::new(Arc::clone(&c.descriptors))),
            Duration::from_secs(600),
        )
        .with_publishing(publishing)
        .with_opener(Arc::new(opener));
    Node {
        role: Arc::new(role),
        committer,
        log,
    }
}

pub(crate) fn create(c: &Cluster, name: &str, uuid: &str) {
    let mut record = IndexRecord::new(name, uuid, 1);
    record.mapping = Some(std::borrow::Cow::Borrowed(MAPPING));
    c.descriptors
        .create(&record, 1, &mut vec![0u8; 1 << 16])
        .expect("created");
}

pub(crate) fn info() -> IndexInfo {
    IndexInfo {
        name: "logs".into(),
        uuid: "u1".into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
        write_block: Default::default(),
        default_pipeline: None,
        final_pipeline: None,
    }
}

pub(crate) fn write(role: &WriterRole, id: &str) -> ItemResult {
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
    let body = format!("{{\"index\":{{\"_id\":\"{id}\"}}}}\n{{\"n\":1}}\n");
    stream(
        &mut body.as_bytes(),
        &mut buffer,
        &defaults,
        &budget,
        config,
        &mut run,
    )
    .expect("streams");
    run.finish().expect("finishes").remove(0).result
}

pub(crate) fn seq(result: &ItemResult) -> i64 {
    match result {
        ItemResult::Written { written, .. } => written.seq_no,
        other => panic!("{other:?}"),
    }
}

/// What node-2 reads of node-1's log: its log store and positions as they
/// are now.
pub(crate) fn takeover(c: &Cluster, from: &Node, calls: &Arc<AtomicUsize>) -> Takeover {
    let log = from.log.clone();
    let epochs = from.committer.positions();
    let calls = Arc::clone(calls);
    Takeover {
        predecessors: Arc::new(Predecessors::new(Box::new(move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Some(Arc::new(NodeLogTakeover::new(
                log.clone(),
                epochs.clone(),
                "node-2",
                2,
            ))))
        }))),
        heads: HeadWriters::new(c.store.clone(), ""),
        max_buffer_bytes: 1 << 20,
    }
}

pub(crate) fn registered_term(c: &Cluster) -> u64 {
    let at = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    let mut buf = vec![0u8; 1 << 16];
    let read = c
        .store
        .read_register(&at.manifest_key("").expect("key"), &mut buf)
        .expect("reads")
        .expect("registered");
    CommitManifest::parse(&buf[..read.len])
        .expect("parses")
        .term
}

/// node-1 writes `a` and publishes it, writes `b` and does not.
fn first_writer(c: &Cluster) -> Node {
    create(c, "logs", "u1");
    let one = node(c, "node-1", 1, false, None);
    assert_eq!(seq(&write(&one.role, "a")), 0);
    one.role.refresh(&info(), 0).expect("published");
    assert_eq!(seq(&write(&one.role, "b")), 1);
    one
}

#[test]
fn a_successor_restores_the_commit_replays_the_log_and_carries_on() {
    let c = cluster();
    let one = first_writer(&c);
    assert_eq!(registered_term(&c), 1);
    let calls = Arc::new(AtomicUsize::new(0));
    let two = node(&c, "node-2", 2, true, Some(takeover(&c, &one, &calls)));
    let next = write(&two.role, "c");
    assert_eq!(seq(&next), 2, "after a (the commit) and b (the log)");
    assert_eq!(registered_term(&c), 2, "b was published under the new term");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_successor_of_a_shard_everything_of_which_is_published_publishes_nothing() {
    let c = cluster();
    create(&c, "logs", "u1");
    let one = node(&c, "node-1", 1, false, None);
    assert_eq!(seq(&write(&one.role, "a")), 0);
    one.role.refresh(&info(), 0).expect("published");
    let before = c.store.meter().count(oss_blob::S3Op::Put);
    let calls = Arc::new(AtomicUsize::new(0));
    let two = node(&c, "node-2", 2, true, Some(takeover(&c, &one, &calls)));
    assert_eq!(seq(&write(&two.role, "b")), 1);
    // The head's take, its settle, the log of the write; no commit.
    assert_eq!(registered_term(&c), 1, "nothing was replayed to publish");
    assert!(c.store.meter().count(oss_blob::S3Op::Put) > before);
}

#[test]
fn a_shard_whose_predecessor_log_cannot_be_read_is_not_held_and_is_tried_again() {
    let c = cluster();
    let one = first_writer(&c);
    let calls = Arc::new(AtomicUsize::new(0));
    let failing = Arc::clone(&calls);
    let broken = Takeover {
        predecessors: Arc::new(Predecessors::new(Box::new(move |_| {
            failing.fetch_add(1, Ordering::SeqCst);
            Err(oss_writer::log::TakeoverError::Lease(Arc::new(
                std::io::Error::other("no lease"),
            )))
        }))),
        heads: HeadWriters::new(c.store.clone(), ""),
        max_buffer_bytes: 1 << 20,
    };
    let _ = one;
    let two = node(&c, "node-2", 2, true, Some(broken));
    for _ in 0..2 {
        match write(&two.role, "c") {
            ItemResult::Failed { status, .. } => assert_eq!(status, 503),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2, "asked again, not held");
}

#[test]
fn a_successor_reads_the_predecessor_log_from_its_lease() {
    let c = cluster();
    let _one = first_writer(&c);
    let two = node(
        &c,
        "node-2",
        2,
        true,
        Some(Takeover::from_leases(
            c.store.clone(),
            "",
            "node-2",
            2,
            1 << 20,
        )),
    );
    assert_eq!(seq(&write(&two.role, "c")), 2, "after a and b");
    assert_eq!(registered_term(&c), 2, "b was published under the new term");
}

#[test]
fn a_predecessor_with_no_lease_register_has_no_log_to_read() {
    let c = cluster();
    let takeover = Takeover::from_leases(c.store.clone(), "", "node-2", 2, 1 << 20);
    let ghost = takeover.predecessors.of("node-9").expect("asked");
    assert!(ghost.is_none(), "no register, no log");
    let known = takeover.predecessors.of("node-1").expect("asked");
    assert!(known.is_some(), "node-1 was enrolled with a lease");
}

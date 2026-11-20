//! docs/specs/node-shard-opener: a writer opens a shard on its first write.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_cluster::heads::{Acquisition, ShardHeadStore};
use oss_cluster::{LeaseConfig, LivenessOracle, Membership, SystemClock};
use oss_format::{FileLocation, IndexRecord, ManifestEntry, NodeLease, ShardRef, encode_manifest};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::descriptor_routes::DescriptorRoutes;
use oss_node::writer::{
    DescriptorIncarnation, HeadAdmission, HeadOpener, MembershipLeaseWrite, RoleTarget, ShardNames,
    ShardOpener, WriterRole,
};
use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, StreamConfig, stream,
};
use oss_writer::log::{CommitterConfig, LeaseKeeper, NodeLogCommitter, NodeLogStore};

const MAPPING: &str = r#"{"properties":{"n":{"type":"integer"}}}"#;

/// An opener that counts what it is asked.
struct Counting {
    inner: HeadOpener,
    calls: Arc<AtomicUsize>,
}

impl ShardOpener for Counting {
    fn open(&self, role: &WriterRole, index: &IndexInfo, shard: u32) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.open(role, index, shard)
    }
}

struct Rig {
    calls: Arc<AtomicUsize>,
    store: Arc<Emulator>,
    role: Arc<WriterRole>,
    heads: Arc<ShardHeadStore>,
    descriptors: Arc<DescriptorStore>,
}

fn rig(opener: bool) -> Rig {
    let store = Arc::new(Emulator::new(64 << 20, 1));
    let dynstore: Arc<dyn BlobStore> = store.clone();
    let clock = Arc::new(SystemClock);
    let membership = Arc::new(
        Membership::new(
            Arc::clone(&dynstore),
            "",
            clock.clone(),
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
    let mut lease = NodeLease::new("node-1", "e1", "a1", 0);
    lease.set_roles(["ingest".to_owned()]);
    let descriptors = Arc::new(
        DescriptorStore::new(Arc::clone(&dynstore), "", DeleteMode::Tombstone)
            .expect("descriptors"),
    );
    let oracle = Arc::new(LivenessOracle::new(Arc::clone(&membership)));
    let heads = Arc::new(
        ShardHeadStore::new(
            Arc::clone(&dynstore),
            "",
            clock,
            30_000,
            Arc::clone(&oracle),
        )
        .expect("heads"),
    );
    let log = NodeLogStore::new(Arc::clone(&dynstore), "", "node-1").expect("names");
    let names = Arc::new(ShardNames::default());
    let keeper = Arc::new(LeaseKeeper::new(
        Box::new(MembershipLeaseWrite::new(Arc::clone(&membership), lease)),
        log.clone(),
        Vec::new(),
    ));
    let committer = Arc::new(NodeLogCommitter::new(
        log,
        1,
        Box::new(Arc::clone(&keeper)),
        Box::new(HeadAdmission::new(
            Arc::clone(&heads),
            Arc::clone(&names),
            "node-1",
            "e1",
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
    let calls = Arc::new(AtomicUsize::new(0));
    let mut role = WriterRole::new(committer, routes)
        .with_names(names)
        .with_incarnation(
            Arc::new(DescriptorIncarnation::new(Arc::clone(&descriptors))),
            Duration::from_secs(600),
        );
    if opener {
        role = role.with_opener(Arc::new(Counting {
            inner: HeadOpener::new(
                Arc::clone(&heads),
                Arc::clone(&descriptors),
                Arc::clone(&dynstore),
                "",
                "node-1",
                "e1",
                1 << 16,
            ),
            calls: Arc::clone(&calls),
        }));
    }
    Rig {
        calls,
        store,
        role: Arc::new(role),
        heads,
        descriptors,
    }
}

fn create(r: &Rig, name: &str, uuid: &str) {
    let mut record = IndexRecord::new(name, uuid, 1);
    record.mapping = Some(std::borrow::Cow::Borrowed(MAPPING));
    r.descriptors
        .create(&record, 1, &mut vec![0u8; 1 << 16])
        .expect("created");
}

/// Writes one document to `index` shard 0 and answers the item.
fn write(role: &WriterRole, index: &str, id: &str) -> ItemResult {
    let defaults = Defaults {
        index: Some(index.into()),
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

fn failed(result: &ItemResult) -> (u16, String) {
    match result {
        ItemResult::Failed { status, error } => (*status, error.reason().to_owned()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_first_write_opens_the_shard_and_a_second_opens_nothing() {
    let r = rig(true);
    create(&r, "logs", "u1");
    let first = write(&r.role, "logs", "1");
    assert!(matches!(first, ItemResult::Written { .. }), "{first:?}");
    let head = r.heads.read("logs", 0).expect("read").expect("a head");
    assert_eq!(head.owner_node_id.as_deref(), Some("node-1"));
    assert!(head.term > 0);
    assert_eq!(r.calls.load(Ordering::SeqCst), 1);
    let second = write(&r.role, "logs", "2");
    assert!(matches!(second, ItemResult::Written { .. }), "{second:?}");
    assert_eq!(r.calls.load(Ordering::SeqCst), 1, "the shard is held");
}

#[test]
fn groups_racing_for_an_unheld_shard_open_it_once() {
    let r = rig(true);
    create(&r, "logs", "u1");
    let term = std::thread::scope(|s| {
        let hs: Vec<_> = (0..4)
            .map(|i| {
                let role = Arc::clone(&r.role);
                s.spawn(move || write(&role, "logs", &i.to_string()))
            })
            .collect();
        for h in hs {
            let result = h.join().expect("joined");
            assert!(matches!(result, ItemResult::Written { .. }), "{result:?}");
        }
        r.heads.read("logs", 0).expect("read").expect("head").term
    });
    assert_eq!(term, 1, "one acquire, the first term");
    assert_eq!(r.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_shard_a_live_node_owns_is_not_opened() {
    let r = rig(true);
    create(&r, "logs", "u1");
    let Acquisition::Won { .. } = r
        .heads
        .acquire("logs", 0, "node-2", "e2", Some("u1"))
        .expect("acquires")
    else {
        panic!("the take");
    };
    let (status, reason) = failed(&write(&r.role, "logs", "1"));
    assert_eq!(status, 503);
    assert!(reason.contains("node-2"), "{reason}");
}

#[test]
fn a_shard_with_a_history_is_refused_as_takeover() {
    let r = rig(true);
    create(&r, "logs", "u1");
    // A published commit.
    let at = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
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
    let (status, reason) = failed(&write(&r.role, "logs", "1"));
    assert_eq!(status, 503);
    assert!(reason.contains("takeover"), "{reason}");
    // A head that names an earlier writer: node-3 held it and has no lease.
    create(&r, "other", "u2");
    // Its claim lapsed the moment it was made.
    ShardHeadStore::new(
        r.store.clone(),
        "",
        Arc::new(SystemClock),
        0,
        Arc::new(LivenessOracle::new(Arc::new(
            Membership::new(
                r.store.clone(),
                "",
                Arc::new(SystemClock),
                LeaseConfig {
                    ttl_millis: 30_000,
                    max_members: 16,
                    conditional_deletes: false,
                },
            )
            .expect("membership"),
        ))),
    )
    .expect("heads")
    .acquire("other", 0, "node-3", "e3", Some("u2"))
    .expect("acquires");
    let (status, reason) = failed(&write(&r.role, "other", "1"));
    assert_eq!(status, 503);
    assert!(reason.contains("takeover"), "{reason}");
}

#[test]
fn an_index_recreated_since_the_group_was_planned_is_not_opened() {
    let r = rig(true);
    create(&r, "logs", "u1");
    // The routes planned the write for uuid u1; the opener's descriptor read
    // finds the same, so open directly with a stale uuid instead.
    let stale = IndexInfo {
        name: "logs".into(),
        uuid: "old".into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
    };
    let opener = HeadOpener::new(
        Arc::clone(&r.heads),
        Arc::clone(&r.descriptors),
        r.store.clone(),
        "",
        "node-1",
        "e1",
        1 << 16,
    );
    let err = opener.open(&r.role, &stale, 0).expect_err("stale");
    assert!(err.contains("no longer the index"), "{err}");
}

#[test]
fn a_role_without_an_opener_says_the_shard_is_not_open() {
    let r = rig(false);
    create(&r, "logs", "u1");
    let (status, reason) = failed(&write(&r.role, "logs", "1"));
    assert_eq!(status, 503);
    assert!(reason.contains("not open on this node"), "{reason}");
}

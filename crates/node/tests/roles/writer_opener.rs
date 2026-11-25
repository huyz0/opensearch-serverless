//! docs/specs/node-shard-opener: a writer opens a shard on its first write.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};

use super::writer_opener_rig::{Counting, Forwarding};
use oss_cluster::heads::{Acquisition, ShardHeadStore};
use oss_cluster::{LeaseConfig, LivenessOracle, Membership, SystemClock};
use oss_format::{FileLocation, IndexRecord, ManifestEntry, NodeLease, ShardRef, encode_manifest};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::descriptor_routes::DescriptorRoutes;
use oss_node::writer::{
    DescriptorIncarnation, HeadAdmission, HeadOpener, MembershipLeaseWrite, ModuleLoader,
    Publishing, RoleTarget, ShardNames, ShardOpener, WriterRole,
};
use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, StreamConfig, stream,
};
use oss_writer::log::{CommitterConfig, LeaseKeeper, NodeLogCommitter, NodeLogStore};
use oss_writer::publish::lazy::Policy;

const MAPPING: &str = r#"{"properties":{"n":{"type":"integer"}}}"#;

pub(super) struct Rig {
    pub(super) forwards: Arc<Forwarding>,
    pub(super) calls: Arc<AtomicUsize>,
    pub(super) store: Arc<Emulator>,
    pub(super) role: Arc<WriterRole>,
    pub(super) heads: Arc<ShardHeadStore>,
    pub(super) descriptors: Arc<DescriptorStore>,
    pub(super) clock: Arc<AtomicU64>,
}

fn rig(opener: bool) -> Rig {
    rig_with(opener, None)
}

pub(super) fn rig_with(opener: bool, modules: Option<Arc<dyn ModuleLoader>>) -> Rig {
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
    let forwards = Arc::new(Forwarding {
        inner: DescriptorRoutes::new(
            DescriptorStore::new(Arc::clone(&dynstore), "", DeleteMode::Tombstone).expect("names"),
            1 << 16,
        ),
        sent: Mutex::new(Vec::new()),
        fail: AtomicBool::new(false),
    });
    let routes = Arc::clone(&forwards);
    let calls = Arc::new(AtomicUsize::new(0));
    let clock = Arc::new(AtomicU64::new(0));
    let ticking = Arc::clone(&clock);
    let mut role = WriterRole::new(committer, routes)
        .with_publishing(Publishing::new(
            Arc::clone(&dynstore),
            "",
            "node-1",
            Policy::new(1_000, 0, 250),
            Box::new(move || ticking.load(Ordering::SeqCst)),
        ))
        .with_names(names)
        .with_incarnation(
            Arc::new(DescriptorIncarnation::new(Arc::clone(&descriptors))),
            Duration::from_secs(600),
        );
    if opener {
        let mut inner = HeadOpener::new(
            Arc::clone(&heads),
            Arc::clone(&descriptors),
            Arc::clone(&dynstore),
            "",
            "node-1",
            "e1",
            1 << 16,
        );
        if let Some(loader) = modules {
            inner = inner.with_modules(loader);
        }
        role = role.with_opener(Arc::new(Counting {
            inner,
            calls: Arc::clone(&calls),
        }));
    }
    Rig {
        forwards,
        calls,
        store,
        role: Arc::new(role),
        heads,
        descriptors,
        clock,
    }
}

fn info(name: &str, uuid: &str) -> IndexInfo {
    IndexInfo {
        name: name.into(),
        uuid: uuid.into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
        write_block: Default::default(),
        default_pipeline: None,
        final_pipeline: None,
    }
}

fn create(r: &Rig, name: &str, uuid: &str) {
    create_with(r, name, uuid, b"{}");
}

fn create_with(r: &Rig, name: &str, uuid: &str, settings: &[u8]) {
    create_mapped(r, name, uuid, settings, MAPPING);
}

pub(super) fn create_mapped(r: &Rig, name: &str, uuid: &str, settings: &[u8], mapping: &str) {
    let mut record = IndexRecord::new(name, uuid, 1);
    record.settings = settings;
    record.mapping = Some(std::borrow::Cow::Owned(mapping.to_owned()));
    r.descriptors
        .create(&record, 1, &mut vec![0u8; 1 << 16])
        .expect("created");
}

/// Writes one document to `index` shard 0 and answers the item.
fn write(role: &WriterRole, index: &str, id: &str) -> ItemResult {
    write_doc(role, index, id, r#"{"n":1}"#)
}

pub(super) fn write_doc(role: &WriterRole, index: &str, id: &str, doc: &str) -> ItemResult {
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
    let body = format!("{{\"index\":{{\"_id\":\"{id}\"}}}}\n{doc}\n");
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

pub(super) fn failed(result: &ItemResult) -> (u16, String) {
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

/// A head `node-2` holds, live.
fn held_by_node_2(r: &Rig) {
    let Acquisition::Won { .. } = r
        .heads
        .acquire("logs", 0, "node-2", "e2", Some("u1"))
        .expect("acquires")
    else {
        panic!("the take");
    };
}

#[test]
fn a_shard_a_live_node_holds_is_not_opened_here_and_its_write_goes_to_that_node() {
    let r = rig(true);
    create(&r, "logs", "u1");
    held_by_node_2(&r);
    let result = write(&r.role, "logs", "1");
    assert!(matches!(result, ItemResult::Written { .. }), "{result:?}");
    assert_eq!(*r.forwards.sent.lock().expect("lock"), ["node-2"]);
    let head = r.heads.read("logs", 0).expect("read").expect("head");
    assert_eq!(head.owner_node_id.as_deref(), Some("node-2"), "not taken");
    assert!(!r.role.held("u1", 0));
}

#[test]
fn a_second_write_goes_to_the_holder_without_reading_the_head() {
    let r = rig(true);
    create(&r, "logs", "u1");
    held_by_node_2(&r);
    write(&r.role, "logs", "1");
    let before = r.store.meter().total();
    let result = write(&r.role, "logs", "2");
    assert!(matches!(result, ItemResult::Written { .. }), "{result:?}");
    assert_eq!(*r.forwards.sent.lock().expect("lock"), ["node-2", "node-2"]);
    assert_eq!(
        r.calls.load(Ordering::SeqCst),
        1,
        "the opener is asked once"
    );
    let after = r.store.meter().total();
    // The one request is the descriptor read that resolves the index name;
    // the head is not read.
    assert_eq!(after - before, 1, "only the index lookup");
}

#[test]
fn a_holder_that_fails_is_asked_again_through_the_head() {
    let r = rig(true);
    create(&r, "logs", "u1");
    held_by_node_2(&r);
    write(&r.role, "logs", "1");
    r.forwards.fail.store(true, Ordering::SeqCst);
    let (status, _) = failed(&write(&r.role, "logs", "2"));
    assert_eq!(status, 503);
    r.forwards.fail.store(false, Ordering::SeqCst);
    let result = write(&r.role, "logs", "3");
    assert!(matches!(result, ItemResult::Written { .. }), "{result:?}");
    assert_eq!(r.calls.load(Ordering::SeqCst), 2, "the head is read again");
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
    let stale = info("logs", "old");
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
    assert!(err.reason.contains("no longer the index"), "{err}");
}

// docs/specs/shared-mappings AC-3
#[test]
fn shards_of_indexes_with_one_mapping_hold_one_parsed_copy() {
    let r = rig(false);
    let opener = HeadOpener::new(
        Arc::clone(&r.heads),
        Arc::clone(&r.descriptors),
        r.store.clone(),
        "",
        "node-1",
        "e1",
        1 << 16,
    );
    for n in 0..3 {
        let (name, uuid) = (format!("logs-{n}"), format!("u{n}"));
        create(&r, &name, &uuid);
        opener.open(&r.role, &info(&name, &uuid), 0).expect("opens");
    }
    assert_eq!(opener.shared_mappings(), 1);
}

#[test]
fn a_role_without_an_opener_says_the_shard_is_not_open() {
    let r = rig(false);
    create(&r, "logs", "u1");
    let (status, reason) = failed(&write(&r.role, "logs", "1"));
    assert_eq!(status, 503);
    assert!(reason.contains("not open on this node"), "{reason}");
}

/// How many shards a pass published once the clock reads `now`.
fn published_at(r: &Rig, now: u64) -> usize {
    r.clock.store(now, Ordering::SeqCst);
    r.role.publish_due().published
}

#[test]
fn an_indexs_refresh_interval_sets_how_long_its_shards_wait_to_publish() {
    for (settings, waits_past_the_nodes_age, name) in [
        (&b"{}"[..], false, "plain"),
        (&br#"{"index":{"refresh_interval":"1h"}}"#[..], true, "hour"),
        (&br#"{"index.refresh_interval":"-1"}"#[..], true, "never"),
    ] {
        let r = rig(true);
        create_with(
            &r,
            name,
            "u1",
            Box::leak(settings.to_vec().into_boxed_slice()),
        );
        assert!(matches!(
            write(&r.role, name, "1"),
            ItemResult::Written { .. }
        ));
        // The first pass publishes a new shard whatever its interval.
        assert_eq!(published_at(&r, 0), 1, "{name}");
        assert!(matches!(
            write(&r.role, name, "2"),
            ItemResult::Written { .. }
        ));
        // Past the node's one second, and well short of an hour.
        let due = published_at(&r, 2_000) == 1;
        assert_eq!(due, !waits_past_the_nodes_age, "{name}");
    }
}

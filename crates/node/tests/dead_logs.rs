//! docs/specs/node-dead-logs: the janitor's collector over the store-backed
//! log and commit seams.

use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_cluster::heads::ShardHeadStore;
use oss_cluster::{LeaseClock, LeaseConfig, LivenessOracle, Membership, SystemClock};
use oss_format::{IndexRecord, NodeLease, ShardRef};
use oss_janitor::node_log::{Collected, Collector};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::descriptor_routes::DescriptorRoutes;
use oss_node::writer::{
    DescriptorIncarnation, HeadAdmission, HeadOpener, MembershipLeaseWrite, Publishing, RoleTarget,
    ShardNames, StoreCommits, StoreNodeLogs, Takeover, WriterRole,
};
use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, StreamConfig, stream,
};
use oss_writer::log::{CommitterConfig, LeaseKeeper, NodeLogCommitter, NodeLogStore};
use oss_writer::publish::lazy::Policy;
use oss_writer::takeover::commit_checkpoint;

const MAPPING: &str = r#"{"properties":{"n":{"type":"integer"}}}"#;
const HOUR: i64 = 3_600_000;

struct Later;

impl LeaseClock for Later {
    fn now_millis(&self) -> i64 {
        SystemClock.now_millis().saturating_add(HOUR)
    }

    fn pause(&self, pause: Duration) {
        SystemClock.pause(pause);
    }
}

fn membership(store: &Arc<Emulator>, clock: Arc<dyn LeaseClock>) -> Arc<Membership> {
    Arc::new(
        Membership::new(
            store.clone(),
            "",
            clock,
            LeaseConfig {
                ttl_millis: 30_000,
                max_members: 16,
                conditional_deletes: false,
            },
        )
        .expect("membership"),
    )
}

fn node(
    store: &Arc<Emulator>,
    descriptors: &Arc<DescriptorStore>,
    id: &str,
    term: u64,
    later: bool,
    takeover: Option<Takeover>,
) -> Arc<WriterRole> {
    let clock: Arc<dyn LeaseClock> = if later {
        Arc::new(Later)
    } else {
        Arc::new(SystemClock)
    };
    let members = membership(store, Arc::clone(&clock));
    let dynstore: Arc<dyn BlobStore> = store.clone();
    let eph = format!("e{}", &id[5..]);
    let heads = Arc::new(
        ShardHeadStore::new(
            Arc::clone(&dynstore),
            "",
            clock,
            30_000,
            Arc::new(LivenessOracle::new(Arc::clone(&members))),
        )
        .expect("heads"),
    );
    let log = NodeLogStore::new(Arc::clone(&dynstore), "", id).expect("names");
    let names = Arc::new(ShardNames::default());
    let mut lease = NodeLease::new(id, &eph, "a1", 0);
    lease.set_roles(["ingest".to_owned()]);
    let keeper = Arc::new(LeaseKeeper::new(
        Box::new(MembershipLeaseWrite::new(members, lease)),
        log.clone(),
        Vec::new(),
    ));
    let committer = Arc::new(NodeLogCommitter::new(
        log,
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
        Arc::clone(descriptors),
        Arc::clone(&dynstore),
        "",
        id,
        &eph,
        1 << 16,
    );
    if let Some(takeover) = takeover {
        opener = opener.with_takeover(takeover);
    }
    let publishing = Publishing::new(dynstore, "", id, Policy::new(0, 0, 250), Box::new(|| 1_000));
    Arc::new(
        WriterRole::new(committer, routes)
            .with_names(names)
            .with_incarnation(
                Arc::new(DescriptorIncarnation::new(Arc::clone(descriptors))),
                Duration::from_secs(600),
            )
            .with_publishing(publishing)
            .with_opener(Arc::new(opener)),
    )
}

fn info() -> IndexInfo {
    IndexInfo {
        name: "logs".into(),
        uuid: "u1".into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
    }
}

fn write(role: &WriterRole, id: &str) -> i64 {
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
    match run.finish().expect("finishes").remove(0).result {
        ItemResult::Written { written, .. } => written.seq_no,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_dead_nodes_log_is_collected_once_a_successor_has_published_all_of_it() {
    let store = Arc::new(Emulator::new(64 << 20, 1));
    let dynstore: Arc<dyn BlobStore> = store.clone();
    let descriptors = Arc::new(
        DescriptorStore::new(Arc::clone(&dynstore), "", DeleteMode::Tombstone)
            .expect("descriptors"),
    );
    let mut record = IndexRecord::new("logs", "u1", 1);
    record.mapping = Some(std::borrow::Cow::Borrowed(MAPPING));
    descriptors
        .create(&record, 1, &mut vec![0u8; 1 << 16])
        .expect("created");

    let one = node(&store, &descriptors, "node-1", 1, false, None);
    assert_eq!(write(&one, "a"), 0);
    one.refresh(&info(), 0).expect("published");
    assert_eq!(write(&one, "b"), 1);

    let later = membership(&store, Arc::new(Later));
    let heads = ShardHeadStore::new(
        Arc::clone(&dynstore),
        "",
        Arc::new(Later),
        30_000,
        Arc::new(LivenessOracle::new(Arc::clone(&later))),
    )
    .expect("heads");
    let logs = StoreNodeLogs::new(Arc::clone(&dynstore), "", "node-2", 2);
    let commits = StoreCommits::new(Arc::clone(&dynstore), "");
    let clock = || SystemClock.now_millis().saturating_add(HOUR);
    let mut buf = vec![0u8; 1 << 16];
    let pass = |dry_run: bool, buf: &mut [u8]| {
        Collector::new(
            &later,
            &heads,
            &descriptors,
            &logs,
            &commits,
            &clock,
            dry_run,
        )
        .collect("node-1", buf)
        .expect("collects")
    };

    // b is only in the log, and the head is still node-1's.
    assert_eq!(pass(false, &mut buf), Collected::StillNeeded);

    let two = node(
        &store,
        &descriptors,
        "node-2",
        2,
        true,
        Some(Takeover::from_leases(
            Arc::clone(&dynstore),
            "",
            "node-2",
            2,
            1 << 20,
        )),
    );
    assert_eq!(write(&two, "c"), 2, "node-2 replayed a and b");

    let at = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    assert_eq!(
        commit_checkpoint(store.as_ref(), "", &at).expect("reads"),
        Some(Some(1)),
        "b was published under the new term"
    );

    assert_eq!(pass(true, &mut buf), Collected::WouldCollect);
    let lists = store.meter().count(S3Op::List);
    assert_eq!(pass(false, &mut buf), Collected::Collected);
    assert_eq!(store.meter().count(S3Op::List), lists, "no LIST to collect");
    assert_eq!(
        pass(false, &mut buf),
        Collected::Nothing,
        "the lease forgot it"
    );
}

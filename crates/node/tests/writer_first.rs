//! docs/specs/request-budgets: the first write after a take, held to
//! docs/request-budgets.md on the emulator: the incarnation read, the
//! lease write that opens the epoch, the shard-head read that admits the
//! shard to it, and the log blob.

use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, Rounds};
use oss_cluster::heads::{Acquisition, ShardHeadStore};
use oss_cluster::{LeaseConfig, LivenessOracle, Membership, SystemClock};
use oss_format::{IndexRecord, NodeLease};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::writer::{
    DescriptorIncarnation, HeadAdmission, MembershipLeaseWrite, RoleTarget, Routes, ShardNames,
    WriterRole,
};
use oss_testkit::budget;
use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, Owner, Planned, StreamConfig,
    stream,
};
use oss_writer::log::{CommitterConfig, LeaseKeeper, NodeLogCommitter, NodeLogStore};
use oss_writer::mapping::Mapping;

const DELAY: Duration = Duration::from_millis(20);

struct Local;

impl Routes for Local {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        Some(info(name))
    }
    fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }
    fn forward(&self, _: &str, _: &IndexInfo, _: u32, _: &[Planned<'_>]) -> Vec<ItemResult> {
        unreachable!("every shard is local")
    }
}

fn info(name: &str) -> IndexInfo {
    IndexInfo {
        name: name.into(),
        uuid: "u1".into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
    }
}

fn write_one(role: &WriterRole, id: &str) {
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
    let items = run.finish().expect("finishes");
    assert!(
        matches!(items[0].result, ItemResult::Written { .. }),
        "{:?}",
        items[0].result
    );
}

#[test]
fn the_first_write_after_a_take_costs_what_the_table_says() {
    let rounds = Arc::new(Rounds::new(Emulator::new(64 << 20, 1)));
    let store: Arc<dyn BlobStore> = rounds.clone();
    let clock = Arc::new(SystemClock);
    let membership = Arc::new(
        Membership::new(
            Arc::clone(&store),
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
    let mut lease = NodeLease::new("node-1", "e1", "a1", 0);
    lease.set_roles(["ingest".to_owned()]);
    membership.renew(&lease).expect("enrolled");

    let descriptors = Arc::new(
        DescriptorStore::new(Arc::clone(&store), "", DeleteMode::Tombstone).expect("descriptors"),
    );
    let mut buf = vec![0u8; 4096];
    descriptors
        .create(&IndexRecord::new("logs", "u1", 1), 1, &mut buf)
        .expect("created");

    let heads = Arc::new(
        ShardHeadStore::new(
            Arc::clone(&store),
            "",
            clock,
            30_000,
            Arc::new(LivenessOracle::new(Arc::clone(&membership))),
        )
        .expect("heads"),
    );
    let Acquisition::Won { head, .. } = heads
        .acquire("logs", 0, "node-1", "e1", Some("u1"))
        .expect("acquires")
    else {
        panic!("the take");
    };

    let log = NodeLogStore::new(Arc::clone(&store), "", "node-1").expect("names");
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
            heads,
            Arc::clone(&names),
            "node-1",
            "e1",
        )),
        CommitterConfig {
            window: Duration::ZERO,
            ..CommitterConfig::default()
        },
    ));
    let role = WriterRole::new(committer, Arc::new(Local))
        .with_names(names)
        .with_incarnation(
            Arc::new(DescriptorIncarnation::new(descriptors)),
            Duration::from_secs(600),
        );
    role.open_shard(
        &info("logs"),
        0,
        head.term,
        Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping"),
    )
    .expect("opens");

    rounds.reset(DELAY);
    let before = rounds.meter().total();
    write_one(&role, "first");
    let detail = rounds.detail();
    budget::check(
        "write.first.requests",
        rounds.meter().total() - before,
        &detail,
    );
    budget::check(
        "write.first.round_trips",
        u64::try_from(rounds.depth()).expect("fits"),
        &detail,
    );

    rounds.reset(DELAY);
    let before = rounds.meter().total();
    write_one(&role, "next");
    let detail = rounds.detail();
    budget::check(
        "write.next.requests",
        rounds.meter().total() - before,
        &detail,
    );
    budget::check(
        "write.next.round_trips",
        u64::try_from(rounds.depth()).expect("fits"),
        &detail,
    );
    rounds.reset(Duration::ZERO);
}

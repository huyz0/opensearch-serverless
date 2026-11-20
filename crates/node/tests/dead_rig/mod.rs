//! Two writer nodes over one emulator, the first of which a clock an hour on
//! sees as dead: the rig of the dead-log tests.

use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_cluster::heads::ShardHeadStore;
use oss_cluster::{LeaseClock, LeaseConfig, LivenessOracle, Membership, SystemClock};
use oss_format::NodeLease;
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::descriptor_routes::DescriptorRoutes;
use oss_node::writer::{
    DescriptorIncarnation, HeadAdmission, HeadOpener, MembershipLeaseWrite, Publishing, RoleTarget,
    ShardNames, Takeover, WriterRole,
};
use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, StreamConfig, stream,
};
use oss_writer::log::{CommitterConfig, LeaseKeeper, NodeLogCommitter, NodeLogStore};
use oss_writer::publish::lazy::Policy;

pub const MAPPING: &str = r#"{"properties":{"n":{"type":"integer"}}}"#;
pub const HOUR: i64 = 3_600_000;

pub struct Later;

impl LeaseClock for Later {
    fn now_millis(&self) -> i64 {
        SystemClock.now_millis().saturating_add(HOUR)
    }

    fn pause(&self, pause: Duration) {
        SystemClock.pause(pause);
    }
}

pub fn membership(store: &Arc<Emulator>, clock: Arc<dyn LeaseClock>) -> Arc<Membership> {
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

pub fn node(
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

pub fn info() -> IndexInfo {
    IndexInfo {
        name: "logs".into(),
        uuid: "u1".into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
    }
}

pub fn write(role: &WriterRole, id: &str) -> i64 {
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

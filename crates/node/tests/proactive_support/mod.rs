//! A rig for the proactive takeover tests: a fleet of writer nodes over one
//! emulator, the first of which writes and dies.

// Each test file uses part of the rig.
#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, Rounds, Shared};
use oss_cluster::heads::ShardHeadStore;
use oss_cluster::{
    LeaseClock, LeaseConfig, LivenessOracle, Membership, MembershipDelta, Refreshed, SystemClock,
};
use oss_format::{CommitManifest, IndexRecord, NodeLease, ShardRef};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::descriptor_routes::DescriptorRoutes;
use oss_node::writer::{
    DescriptorIncarnation, HeadAdmission, HeadOpener, MembershipLeaseWrite, ProactiveConfig,
    Publishing, RoleTarget, ShardNames, Takeover, WriterRole,
};
use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, StreamConfig, stream,
};
use oss_writer::log::{
    CommitterConfig, LeaseKeeper, NodeLogCommitter, NodeLogStore, NodeLogTakeover,
};
use oss_writer::publish::lazy::Policy;
use oss_writer::takeover::{HeadWriters, Predecessors};

mod rustfs;
mod sweep;
#[allow(unused_imports)]
pub use rustfs::{RustFs, cluster_rustfs, cluster_rustfs_rounds};
#[allow(unused_imports)]
pub use sweep::*;

pub const MAPPING: &str = r#"{"properties":{"n":{"type":"integer"}}}"#;

/// The clock an hour on: every lease node-1 holds has lapsed by it.
pub struct Later;

impl LeaseClock for Later {
    fn now_millis(&self) -> i64 {
        SystemClock.now_millis().saturating_add(3_600_000)
    }

    fn pause(&self, pause: Duration) {
        SystemClock.pause(pause);
    }
}

pub struct Cluster {
    pub store: Arc<Emulator>,
    pub descriptors: Arc<DescriptorStore>,
    /// Set by [`cluster_rounds`]: every node reaches the emulator through
    /// this wrapper, which measures round-trip depth.
    pub rounds: Option<Arc<Rounds<Shared>>>,
    /// Set by [`cluster_rustfs_rounds`]: the store is RustFS, and `store`
    /// above is an unused emulator.
    pub external: Option<Arc<dyn BlobStore>>,
    pub _rustfs: Option<RustFs>,
}

impl Cluster {
    /// The store a node talks to: the round counter when there is one.
    pub fn io(&self) -> Arc<dyn BlobStore> {
        match &self.rounds {
            Some(rounds) => rounds.clone(),
            None => self.base(),
        }
    }

    /// The store under any round counter: RustFS or the emulator.
    pub fn base(&self) -> Arc<dyn BlobStore> {
        match &self.external {
            Some(store) => store.clone(),
            None => self.store.clone(),
        }
    }
}

pub struct Node {
    pub role: Arc<WriterRole>,
    pub committer: Arc<NodeLogCommitter>,
    pub log: NodeLogStore,
}

pub fn cluster() -> Cluster {
    cluster_of(64 << 20)
}

/// A cluster whose emulator holds `bytes`.
pub fn cluster_of(bytes: u64) -> Cluster {
    let store = Arc::new(Emulator::new(bytes, 1));
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
    for (node, eph) in [("node-1", "e1"), ("node-2", "e2"), ("node-3", "e3")] {
        let mut lease = NodeLease::new(node, eph, "a", 0);
        lease.set_roles(["ingest".to_owned()]);
        membership.renew(&lease).expect("enrolled");
    }
    let descriptors = Arc::new(
        DescriptorStore::new(Arc::clone(&dynstore), "", DeleteMode::Tombstone)
            .expect("descriptors"),
    );
    Cluster {
        store,
        descriptors,
        rounds: None,
        external: None,
        _rustfs: None,
    }
}

/// A cluster whose nodes reach the store through a round counter.
pub fn cluster_rounds() -> Cluster {
    let mut c = cluster();
    let store: Arc<dyn BlobStore> = c.store.clone();
    let rounds = Arc::new(Rounds::new(Shared::new(store)));
    c.descriptors = Arc::new(
        DescriptorStore::new(rounds.clone(), "", DeleteMode::Tombstone).expect("descriptors"),
    );
    c.rounds = Some(rounds);
    c
}

/// A node `id`; one that is `later` judges leases and head claims an hour on,
/// so it sees every other node as dead.
pub fn node(c: &Cluster, id: &str, term: u64, later: bool, takeover: Option<Takeover>) -> Node {
    node_routed(c, id, term, later, takeover, |routes| Arc::new(routes))
}

/// As [`node`], with the role's routes made from the descriptor routes by
/// `wrap`: a test's own owners and forwards.
pub fn node_routed(
    c: &Cluster,
    id: &str,
    term: u64,
    later: bool,
    takeover: Option<Takeover>,
    wrap: impl FnOnce(DescriptorRoutes) -> Arc<dyn oss_node::writer::Routes>,
) -> Node {
    let check = Arc::new(DescriptorIncarnation::new(Arc::clone(&c.descriptors)));
    node_checked(
        c,
        id,
        term,
        later,
        takeover,
        wrap,
        (check, Duration::from_secs(600)),
    )
}

/// As [`node_routed`], with the incarnation check and its window a test's own.
#[allow(clippy::too_many_arguments)]
pub fn node_checked(
    c: &Cluster,
    id: &str,
    term: u64,
    later: bool,
    takeover: Option<Takeover>,
    wrap: impl FnOnce(DescriptorRoutes) -> Arc<dyn oss_node::writer::Routes>,
    incarnation: (Arc<dyn oss_node::writer::Incarnation>, Duration),
) -> Node {
    let clock: Arc<dyn LeaseClock> = if later {
        Arc::new(Later)
    } else {
        Arc::new(SystemClock)
    };
    let membership = Arc::new(
        Membership::new(
            c.io(),
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
    let dynstore: Arc<dyn BlobStore> = c.io();
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
    let routes = wrap(DescriptorRoutes::new(
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
    let publishing = Publishing::new(dynstore, "", id, Policy::new(0, 0, 250), Box::new(|| 1_000));
    let role = WriterRole::new(Arc::clone(&committer), routes)
        .with_names(names)
        .with_incarnation(incarnation.0, incarnation.1)
        .with_publishing(publishing)
        .with_opener(Arc::new(opener));
    Node {
        role: Arc::new(role),
        committer,
        log,
    }
}

pub fn create(c: &Cluster, name: &str, uuid: &str) {
    create_n(c, name, uuid, 1);
}

/// An index of `shards` shards.
pub fn create_n(c: &Cluster, name: &str, uuid: &str, shards: u32) {
    let mut record = IndexRecord::new(name, uuid, shards);
    record.mapping = Some(std::borrow::Cow::Borrowed(MAPPING));
    c.descriptors
        .create(&record, 1, &mut vec![0u8; 1 << 16])
        .expect("created");
}

pub fn info() -> IndexInfo {
    info_n(1)
}

pub fn info_n(shards: u32) -> IndexInfo {
    IndexInfo {
        name: "logs".into(),
        uuid: "u1".into(),
        shards,
        partition_size: 1,
        routing_shards: shards,
        routing_required: false,
        default_pipeline: None,
        final_pipeline: None,
    }
}

pub fn write(role: &WriterRole, id: &str) -> ItemResult {
    write_in(role, "logs", id)
}

pub fn write_in(role: &WriterRole, index: &str, id: &str) -> ItemResult {
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

pub fn seq(result: &ItemResult) -> i64 {
    match result {
        ItemResult::Written { written, .. } => written.seq_no,
        other => panic!("{other:?}"),
    }
}

/// What node-2 reads of node-1's log: its log store and positions as they
/// are now.
pub fn takeover(c: &Cluster, from: &Node, calls: &Arc<AtomicUsize>) -> Takeover {
    takeover_as(c, from, "node-2", 2, calls)
}

/// What node `id` reads of `from`'s log, fencing it at `term`.
pub fn takeover_as(
    c: &Cluster,
    from: &Node,
    id: &str,
    term: i64,
    calls: &Arc<AtomicUsize>,
) -> Takeover {
    let id = id.to_owned();
    let log = from.log.clone();
    let epochs = from.committer.positions();
    let calls = Arc::clone(calls);
    Takeover {
        predecessors: Arc::new(Predecessors::new(Box::new(move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Some(Arc::new(NodeLogTakeover::new(
                log.clone(),
                epochs.clone(),
                &id,
                term,
            ))))
        }))),
        heads: HeadWriters::new(c.base(), ""),
        max_buffer_bytes: 1 << 20,
    }
}

pub fn registered_term(c: &Cluster) -> u64 {
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
pub fn first_writer(c: &Cluster) -> Node {
    create(c, "logs", "u1");
    let one = node(c, "node-1", 1, false, None);
    assert_eq!(seq(&write(&one.role, "a")), 0);
    one.role.refresh(&info(), 0).expect("published");
    assert_eq!(seq(&write(&one.role, "b")), 1);
    one
}

/// A pass pinned at four at once for 30 s: the old defaults, which these
/// suites keep covering; the shipped defaults are 16 and 60 s.
pub fn config(enabled: bool) -> ProactiveConfig {
    ProactiveConfig {
        enabled,
        reapers: 2,
        max_concurrent: 4,
        pass: Duration::from_secs(30),
        stagger_millis: 10_000,
        rust_reapers_only: false,
    }
}

/// node-1's lease as a refresh reports it: dead, naming the epochs it logged.
pub fn dead(one: &Node) -> NodeLease {
    let mut lease = NodeLease::new("node-1", "e1", "a", 0);
    lease.set_roles(["ingest".to_owned()]);
    lease.node_log = one.committer.positions();
    lease.expires_at_millis = 1_000;
    lease
}

pub fn refreshed(live: &[&str], dead: NodeLease) -> Refreshed {
    let live: Vec<NodeLease> = live
        .iter()
        .map(|id| {
            let mut lease = NodeLease::new(id, "e", "a", 0);
            lease.set_roles(["ingest".to_owned()]);
            lease.expires_at_millis = i64::MAX;
            lease
        })
        .collect();
    Refreshed {
        live: live.into(),
        delta: MembershipDelta::default(),
        dead_with_epochs: vec![dead],
    }
}

/// Ids that route to each shard of an `shards`-shard index, `per` of each,
/// found by writing each candidate to a scratch node.
pub fn ids_by_shard(shards: u32, per: usize) -> Vec<Vec<String>> {
    let mut found: Vec<Vec<String>> = vec![Vec::new(); shards as usize];
    let mut n = 0u32;
    while found.iter().any(|ids| ids.len() < per) {
        assert!(n < 2_000, "no ids reach every shard");
        let id = format!("d{n}");
        n = n.saturating_add(1);
        let c = cluster();
        create_n(&c, "logs", "u1", shards);
        let scratch = node(&c, "node-1", 1, false, None);
        write(&scratch.role, &id);
        let shard = scratch.role.held_shards()[0].shard as usize;
        if found[shard].len() < per {
            found[shard].push(id);
        }
    }
    found
}

/// Writes `id`, which routes to some shard, through `role`.
pub fn write_id(role: &WriterRole, id: &str) -> i64 {
    seq(&write(role, id))
}

/// A janitor pass over `role`'s shards in the cluster's store, as a node
/// builds it, with a clock the pass reads as 5,000 ms.
pub fn janitor(c: &Cluster, role: &Arc<WriterRole>) -> oss_node::writer::JanitorPass {
    use std::sync::atomic::AtomicU64;

    use oss_mem::{BudgetConfig, MemoryBudget};
    use oss_meta::PointInTimeStore;
    use oss_node::writer::{JanitorPass, PassLimits};

    let clock = Arc::new(AtomicU64::new(1_000));
    let pits = PointInTimeStore::new(c.base(), "", Arc::new(move || clock.load(Ordering::SeqCst)))
        .expect("pits");
    let budget = MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(oss_mem::Pool::RequestBodies, 1 << 20)
            .expect("fits"),
    );
    JanitorPass::new(
        c.base(),
        "",
        Arc::clone(role),
        pits,
        &budget,
        PassLimits {
            buffer_bytes: 1 << 16,
            max_views: 100,
        },
        Box::new(|| 5_000),
    )
}

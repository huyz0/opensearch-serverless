//! A proactive-takeover node that packs its small segments.

use super::*;

/// As [`node_checked`], the node packing its small segments when `packs` is
/// set.
#[allow(clippy::too_many_arguments)]
pub fn node_with_packs(
    c: &Cluster,
    id: &str,
    term: u64,
    later: bool,
    takeover: Option<Takeover>,
    wrap: impl FnOnce(DescriptorRoutes) -> Arc<dyn oss_node::writer::Routes>,
    incarnation: (Arc<dyn oss_node::writer::Incarnation>, Duration),
    packs: Option<oss_node::writer::PackConfig>,
) -> Node {
    let clock: Arc<dyn LeaseClock> = if later {
        Arc::new(Later)
    } else {
        Arc::new(SystemClock)
    };
    node_with_clock(
        c,
        id,
        term,
        clock,
        Box::new(|| 1_000),
        takeover,
        wrap,
        incarnation,
        packs,
    )
}

/// As [`node_with_packs`], the node judging leases, head claims and its
/// publishes by `clock`, which a test steps, and its publish policy by `publish_clock`.
#[allow(clippy::too_many_arguments)]
pub fn node_with_clock(
    c: &Cluster,
    id: &str,
    term: u64,
    clock: Arc<dyn LeaseClock>,
    publish_clock: Box<dyn Fn() -> u64 + Send + Sync>,
    takeover: Option<Takeover>,
    wrap: impl FnOnce(DescriptorRoutes) -> Arc<dyn oss_node::writer::Routes>,
    incarnation: (Arc<dyn oss_node::writer::Incarnation>, Duration),
    packs: Option<oss_node::writer::PackConfig>,
) -> Node {
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
    let mut publishing = Publishing::new(dynstore, "", id, Policy::new(0, 0, 250), publish_clock);
    if let Some(packs) = packs {
        publishing = publishing.with_packs(packs);
    }
    let role = WriterRole::new(Arc::clone(&committer), routes)
        .with_names(names)
        .with_incarnation(incarnation.0, incarnation.1)
        .with_publishing(publishing)
        .with_opener(Arc::new(opener));
    Node {
        role: Arc::new(role),
        committer,
        log,
        keeper,
    }
}

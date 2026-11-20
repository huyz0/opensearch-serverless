//! docs/specs/node-cluster-routes: the health source over the stores.

#[path = "cluster_support/mod.rs"]
mod cluster_support;

use std::sync::Arc;

use cluster_support::{index, rig, sent};
use oss_blob::{BlobStore, S3Op};
use oss_cluster::heads::{ShardHeadStore, shard_head_key};
use oss_cluster::{LeaseConfig, LivenessOracle, Membership, SystemClock};
use oss_coordinator::cluster::health::{HeadRead, IndexInput};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::health_route::{HealthSource, StoreHealth};

fn store_health(store: &Arc<dyn BlobStore>, claim_millis: i64) -> (StoreHealth, ShardHeadStore) {
    let clock = Arc::new(SystemClock);
    let membership = Arc::new(
        Membership::new(
            Arc::clone(store),
            "",
            clock.clone(),
            LeaseConfig {
                ttl_millis: 30_000,
                max_members: 8,
                conditional_deletes: false,
            },
        )
        .expect("membership"),
    );
    let oracle = Arc::new(LivenessOracle::new(Arc::clone(&membership)));
    let heads = |ttl| {
        ShardHeadStore::new(
            Arc::clone(store),
            "",
            clock.clone(),
            ttl,
            Arc::clone(&oracle),
        )
        .expect("heads")
    };
    let source = StoreHealth::new(
        membership,
        Arc::clone(&oracle),
        heads(claim_millis),
        DescriptorStore::new(Arc::clone(store), "", DeleteMode::Conditional).expect("names"),
        clock.clone(),
    );
    (source, heads(claim_millis))
}

#[test]
fn the_store_source_reads_the_descriptor_and_a_head_a_shard() {
    let r = rig();
    index(&r, "logs", 4);
    let (live, heads) = store_health(&r.store, 30_000);
    assert_eq!(live.nodes(), Some(0), "nobody has enrolled");
    heads
        .acquire("logs", 0, "n1", "e1", Some("logs-uuid"))
        .expect("won");
    r.store
        .create_register(&shard_head_key("", "logs", 2), b"not a head")
        .expect("junk");
    assert_eq!(
        live.index("logs"),
        IndexInput::Shards(vec![
            (0, HeadRead::Owned { held: true }),
            (1, HeadRead::Unowned),
            (2, HeadRead::Unreadable),
            (3, HeadRead::Unowned),
        ])
    );
    assert_eq!(live.index("nope"), IndexInput::Missing);
}

#[test]
fn a_lapsed_claim_by_an_owner_with_no_lease_is_dormant_and_asks_once_an_owner() {
    let r = rig();
    index(&r, "logs", 3);
    // A claim that has already lapsed, by a node that never enrolled.
    let (lapsed, heads) = store_health(&r.store, -1_000);
    for shard in 0..3 {
        heads
            .acquire("logs", shard, "n1", "e1", Some("logs-uuid"))
            .expect("won");
    }
    let (reads, sent) = sent(&r, || lapsed.index("logs"));
    assert_eq!(
        reads,
        IndexInput::Shards(vec![
            (0, HeadRead::Owned { held: false }),
            (1, HeadRead::Owned { held: false }),
            (2, HeadRead::Owned { held: false }),
        ])
    );
    // A GET a head, and one lease read for the one owner of the three.
    let gets = sent
        .iter()
        .find(|(op, _)| *op == S3Op::Get)
        .map_or(0, |(_, n)| *n);
    assert_eq!(
        gets,
        3 + 1 + 1,
        "descriptor, three heads, one lease read: {sent:?}"
    );
}

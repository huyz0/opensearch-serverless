//! docs/specs/node-lease-refresh AC-11: a node that joins is seen by the nodes
//! already there, and sees them, long before the lease read period.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

mod suite_rig;

use std::time::{Duration, Instant};

use suite_rig::{Cluster, wait_for};

/// The default lease lifetime: a read every third of it is 10 s.
const TTL_MS: u64 = 30_000;
/// Far inside the 10 s read period.
const PICKUP: Duration = Duration::from_secs(4);

#[test]
fn a_joiner_is_seen_by_every_node_inside_the_read_period() {
    let mut cluster = Cluster::new(TTL_MS, 1, false).expect("emulated");
    cluster.add("n1");
    cluster.add("n2");
    let started = Instant::now();
    wait_for("both see two", 60, || {
        (0..2).all(|s| cluster.nodes_seen(s) == 2)
    });
    let first = started.elapsed();
    let joined = Instant::now();
    cluster.add("n3");
    wait_for("all see three", 60, || {
        (0..3).all(|s| cluster.nodes_seen(s) == 3)
    });
    let second = joined.elapsed();
    assert!(
        second < PICKUP,
        "a join took {second:?} to be seen, the read period is 10 s"
    );
    assert!(first < PICKUP + Duration::from_secs(20), "{first:?}");
}

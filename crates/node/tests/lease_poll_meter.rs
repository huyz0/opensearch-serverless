//! docs/specs/node-lease-refresh: what reading the fleet's leases more often
//! costs a node and buys it.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: counts and millisecond figures are small.
#![allow(clippy::arithmetic_side_effects, clippy::cast_precision_loss)]

mod suite_rig;

use std::time::{Duration, Instant};

use oss_blob::S3Op;
use suite_rig::{Cluster, wait_for};

/// The default lease lifetime: a refresh every third of it is 10 s.
const TTL_MS: u64 = 30_000;
/// How long the idle nodes are counted.
const IDLE: Duration = Duration::from_secs(40);
/// Nodes that join one after another after the three that start.
const JOINERS: usize = 3;

fn requests(cluster: &Cluster) -> Vec<(S3Op, u64)> {
    S3Op::ALL
        .iter()
        .map(|op| (*op, cluster.sent(*op)))
        .collect()
}

fn run(refresh_ms: Option<u64>) {
    let mut cluster = Cluster::new(TTL_MS, 1, false).expect("emulated");
    if let Some(ms) = refresh_ms {
        cluster.set("OSS_LEASE_REFRESH_MS", &ms.to_string());
    }
    for id in ["n1", "n2", "n3"] {
        cluster.add(id);
    }
    wait_for("every node sees three", 60, || {
        (0..3).all(|s| cluster.nodes_seen(s) == 3)
    });
    // Idle: nothing but the lease keeping and the fleet view.
    let before = requests(&cluster);
    std::thread::sleep(IDLE);
    let per_node_hour = |now: u64, was: u64| (now - was) as f64 / 3.0 * 3600.0 / IDLE.as_secs_f64();
    let after = requests(&cluster);
    let rates: Vec<String> = before
        .iter()
        .zip(&after)
        .filter(|(b, a)| a.1 > b.1)
        .map(|(b, a)| format!("{:?} {:.0}", b.0, per_node_hour(a.1, b.1)))
        .collect();
    // Joining: how long until the nodes already there all see the new one.
    let mut seen_after = Vec::new();
    for (n, id) in ["n4", "n5", "n6"].iter().enumerate().take(JOINERS) {
        let started = Instant::now();
        cluster.add(id);
        let want = 4 + n as u64;
        wait_for("the joiner is seen by every node", 120, || {
            (0..cluster.live().len()).all(|s| cluster.nodes_seen(s) == want)
        });
        seen_after.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    println!(
        "LEASEMETER|refresh={}|idle per node per hour: {}|join seen by all after (ms): {:?}|provisional: emulator",
        refresh_ms.map_or("default".to_owned(), |m| format!("{m} ms")),
        rates.join(" "),
        seen_after
            .iter()
            .map(|ms| ms.round() as u64)
            .collect::<Vec<_>>()
    );
}

#[test]
#[ignore = "slow"]
fn reading_leases_more_often_costs_requests_and_shortens_how_long_a_join_goes_unseen() {
    // `LEASE_POLL_MS` names the interval to meter: unset, the default.
    let refresh = std::env::var("LEASE_POLL_MS")
        .ok()
        .and_then(|v| v.parse().ok());
    run(refresh);
}

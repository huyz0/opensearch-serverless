//! docs/specs/node-bucket-cost: the requests a second node makes to take over
//! the small indices of a node that died, with the buckets switch off and on.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: counts are small and a later one is never below an earlier one.
#![allow(clippy::arithmetic_side_effects)]

mod suite_rig;

use std::time::{Duration, Instant};

use oss_blob::S3Op;
use suite_rig::{Cluster, wait_for};

const INDICES: usize = 24;

fn counts(cluster: &Cluster) -> Vec<u64> {
    S3Op::ALL.iter().map(|op| cluster.sent(*op)).collect()
}

fn show(label: &str, c: &[u64], took: Duration) {
    let named: Vec<String> = S3Op::ALL
        .iter()
        .zip(c)
        .filter(|(_, n)| **n > 0)
        .map(|(op, n)| format!("{op:?}={n}"))
        .collect();
    println!("{label}: {} in {took:?}", named.join(" "));
}

/// Writes one document to every index through `slot`, retrying each until
/// the node owns it, and returns when all are written.
fn write_all(cluster: &Cluster, slot: usize, rounds: &str) {
    for i in 0..INDICES {
        let started = Instant::now();
        loop {
            let put = cluster.call(
                slot,
                "PUT",
                &format!("/small-{i}/_doc/{rounds}"),
                Some(r#"{"n":1}"#),
            );
            if put.status == 201 {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(90),
                "small-{i}: {} {}",
                put.status,
                put.text()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// The requests of the second node taking over `INDICES` indices, and how
/// long it took.
fn measure(buckets: bool) -> (Vec<u64>, Duration) {
    let mut cluster = Cluster::new(3_000, 50, false).expect("emulator");
    if buckets {
        cluster.set("OSS_BUCKETS_ENABLED", "true");
    }
    cluster.add("n1");
    for i in 0..INDICES {
        let made = cluster.call(0, "PUT", &format!("/small-{i}"), None);
        assert_eq!(made.status, 200, "{}", made.text());
    }
    write_all(&cluster, 0, "1");
    cluster.add("n2");
    wait_for("both nodes see two", 30, || cluster.nodes_seen(1) == 2);
    cluster.kill(0);
    let before = counts(&cluster);
    let started = Instant::now();
    write_all(&cluster, 1, "2");
    let took = started.elapsed();
    let now = counts(&cluster);
    (now.iter().zip(&before).map(|(a, b)| a - b).collect(), took)
}

#[test]
fn taking_over_small_indices_costs_fewer_requests_in_buckets() {
    let (off, off_took) = measure(false);
    let (on, on_took) = measure(true);
    show("takeover off", &off, off_took);
    show("takeover on ", &on, on_took);
    let put = |c: &[u64]| c[S3Op::ALL.iter().position(|o| *o == S3Op::Put).unwrap()];
    let total = |c: &[u64]| c.iter().sum::<u64>();
    // One bucket swap moves every member it holds, where each shard moves alone.
    assert!(put(&on) < put(&off), "{off:?} {on:?}");
    assert!(total(&on) < total(&off), "{off:?} {on:?}");
}

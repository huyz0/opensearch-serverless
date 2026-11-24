//! docs/specs/node-bucket-cost: the requests a node makes for small indices,
//! with the buckets switch off and on, through REST only.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: counts are small and a later one is never below an earlier one.
#![allow(clippy::arithmetic_side_effects)]

mod suite_rig;

use std::time::Duration;

use oss_blob::S3Op;
use suite_rig::{Cluster, wait_for};

const INDICES: usize = 24;

/// Every request kind's count, in `S3Op::ALL` order.
fn counts(cluster: &Cluster) -> Vec<u64> {
    S3Op::ALL.iter().map(|op| cluster.sent(*op)).collect()
}

fn minus(now: &[u64], before: &[u64]) -> Vec<u64> {
    now.iter().zip(before).map(|(a, b)| a - b).collect()
}

fn show(label: &str, c: &[u64]) {
    let named: Vec<String> = S3Op::ALL
        .iter()
        .zip(c)
        .filter(|(_, n)| **n > 0)
        .map(|(op, n)| format!("{op:?}={n}"))
        .collect();
    println!("{label}: {}", named.join(" "));
}

/// The requests of creating, writing and searching `INDICES` one-shard
/// indices on one node, then of it sitting idle.
fn measure(buckets: bool) -> (Vec<u64>, Vec<u64>, Vec<u64>, Vec<u64>) {
    let mut cluster = Cluster::new(3_000, 50, false).expect("emulator");
    if buckets {
        cluster.set("OSS_BUCKETS_ENABLED", "true");
    }
    cluster.add("n1");
    wait_for("the node sees itself", 30, || cluster.nodes_seen(0) == 1);
    std::thread::sleep(Duration::from_millis(3_500));
    let start = counts(&cluster);
    for i in 0..INDICES {
        let made = cluster.call(0, "PUT", &format!("/small-{i}"), None);
        assert_eq!(made.status, 200, "{}", made.text());
    }
    let created = counts(&cluster);
    for i in 0..INDICES {
        let put = cluster.call(
            0,
            "PUT",
            &format!("/small-{i}/_doc/1?refresh=true"),
            Some(r#"{"n":1}"#),
        );
        assert_eq!(put.status, 201, "{}", put.text());
    }
    let written = counts(&cluster);
    for i in 0..INDICES {
        let found = cluster.call(
            0,
            "POST",
            &format!("/small-{i}/_search"),
            Some(r#"{"query":{"match_all":{}}}"#),
        );
        assert_eq!(found.status, 200, "{}", found.text());
        assert!(
            found.text().contains(r#""total":{"value":1"#),
            "{}",
            found.text()
        );
    }
    let searched = counts(&cluster);
    std::thread::sleep(Duration::from_secs(8));
    let idle = counts(&cluster);
    (
        minus(&created, &start),
        minus(&written, &created),
        minus(&searched, &written),
        minus(&idle, &searched),
    )
}

fn total(c: &[u64]) -> u64 {
    c.iter().sum()
}

#[test]
fn small_indices_cost_no_more_requests_in_buckets_once_created() {
    let off = measure(false);
    let on = measure(true);
    for (label, (a, b)) in [
        ("create", (&off.0, &on.0)),
        ("write", (&off.1, &on.1)),
        ("search", (&off.2, &on.2)),
        ("idle 8s", (&off.3, &on.3)),
    ] {
        show(&format!("{label} off"), a);
        show(&format!("{label} on "), b);
    }
    // A search reads the same blobs from a bucket as from a shard register.
    assert_eq!(total(&off.2), total(&on.2));
    // A write and an idle stretch stay within a tenth.
    let near = |a: &[u64], b: &[u64]| total(b) * 10 <= total(a) * 11 + 4;
    assert!(near(&off.1, &on.1), "write: {off:?} {on:?}");
    assert!(near(&off.3, &on.3), "idle: {off:?} {on:?}");
    // A create costs a join: the open bucket's register, the manifest, one swap.
    assert!(total(&on.0) > total(&off.0), "create: {off:?} {on:?}");
    assert!(
        total(&on.0) <= total(&off.0) * 4,
        "a create costs at most three more requests than its descriptor"
    );
}

//! docs/specs/node-bucket-cost: the requests a second node makes to take over
//! the small indices of a node that died, with the buckets switch off and on.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: counts are small and a later one is never below an earlier one.
#![allow(clippy::arithmetic_side_effects)]

mod suite_rig;

use std::time::{Duration, Instant};

use oss_blob::S3Op;
use suite_rig::{Cluster, Reply, wait_for};

const INDICES: usize = 24;

/// The tenants the grouped run spreads the indices over.
const TENANTS: usize = 4;

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

fn search(cluster: &Cluster, slot: usize, target: &str) -> Reply {
    cluster.call(
        slot,
        "POST",
        &format!("{target}/_search"),
        Some(r#"{"query":{"match_all":{}}}"#),
    )
}

/// Every index answers with the document written before the takeover and the
/// one written after it: the taken-over shard restored its commit.
fn keeps_both_documents(cluster: &Cluster, slot: usize) {
    for i in 0..INDICES {
        let target = format!("/small-{i}");
        let refreshed = cluster.call(slot, "POST", &format!("{target}/_refresh"), None);
        assert_eq!(refreshed.status, 200, "{}", refreshed.text());
        // A shard just taken over may answer 503 `shard_behind_exception`
        // until it has caught up with the writes its writer acknowledged.
        let mut found = search(cluster, slot, &target);
        for _ in 0..100 {
            if found.status != 503 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            found = search(cluster, slot, &target);
        }
        assert_eq!(found.status, 200, "{}", found.text());
        let text = found.text();
        assert!(text.contains(r#""total":{"value":2"#), "small-{i}: {text}");
    }
}

/// The requests of the second node taking over `INDICES` indices, and how
/// long it took.
fn measure(buckets: bool, grouped: bool) -> (Vec<u64>, Duration) {
    let mut cluster = Cluster::new(3_000, 50, false).expect("emulator");
    if buckets {
        cluster.set("OSS_BUCKETS_ENABLED", "true");
    }
    cluster.add("n1");
    for i in 0..INDICES {
        let body = format!(
            r#"{{"settings":{{"index.serverless.bucket_group":"tenant-{}"}}}}"#,
            i % TENANTS
        );
        let made = cluster.call(
            0,
            "PUT",
            &format!("/small-{i}"),
            grouped.then_some(body.as_str()),
        );
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
    keeps_both_documents(&cluster, 1);
    (now.iter().zip(&before).map(|(a, b)| a - b).collect(), took)
}

#[test]
fn taking_over_small_indices_costs_fewer_requests_in_buckets() {
    // Three clusters of their own, each with its own store: side by side,
    // the test takes as long as the slowest of them.
    let ((off, off_took), (on, on_took), (own, own_took)) = std::thread::scope(|s| {
        let off = s.spawn(|| measure(false, true));
        let on = s.spawn(|| measure(true, true));
        let own = measure(true, false);
        (
            off.join().expect("the buckets-off run"),
            on.join().expect("the tenant-grouped run"),
            own,
        )
    });
    show("takeover off      ", &off, off_took);
    show("takeover on, tenants", &on, on_took);
    show("takeover on, own group", &own, own_took);
    let put = |c: &[u64]| c[S3Op::ALL.iter().position(|o| *o == S3Op::Put).unwrap()];
    let total = |c: &[u64]| c.iter().sum::<u64>();
    // One bucket swap moves every member it holds, where each shard moves alone.
    assert!(put(&on) < put(&off), "{off:?} {on:?}");
    assert!(total(&on) < total(&off), "{off:?} {on:?}");
}

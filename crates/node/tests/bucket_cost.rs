//! docs/specs/node-bucket-cost: the requests a node makes for small indices,
//! with the buckets switch off and on, through REST only.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: counts are small and a later one is never below an earlier one.
#![allow(clippy::arithmetic_side_effects)]

mod suite_rig;

use std::time::{Duration, Instant};

use oss_blob::S3Op;
use suite_rig::{Cluster, wait_for};

const INDICES: usize = 24;

/// Rounds of timed writes, each one document to every index.
const WRITE_ROUNDS: usize = 4;

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

/// The request counts of create, write, search and idle, then each timed write.
type Measured = (Vec<u64>, Vec<u64>, Vec<u64>, Vec<u64>, Vec<Duration>);

/// The requests of creating, writing and searching `INDICES` one-shard
/// indices on one node, then of it sitting idle.
fn measure(buckets: bool) -> Measured {
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
    // Further documents with refresh on, each write timed.
    let mut times = Vec::new();
    for round in 2..=WRITE_ROUNDS + 1 {
        for i in 0..INDICES {
            let started = Instant::now();
            let put = cluster.call(
                0,
                "PUT",
                &format!("/small-{i}/_doc/{round}?refresh=true"),
                Some(r#"{"n":1}"#),
            );
            times.push(started.elapsed());
            assert_eq!(put.status, 201, "{}", put.text());
        }
    }
    times.sort();
    (
        minus(&created, &start),
        minus(&written, &created),
        minus(&searched, &written),
        minus(&idle, &searched),
        times,
    )
}

fn total(c: &[u64]) -> u64 {
    c.iter().sum()
}

/// A search reads no more blobs from a bucket than from a shard register.
///
/// The ranged reads and the writes are exact: only a search makes them. A
/// plain read is not: the node re-reads the transport secrets from the store
/// once every lease TTL (3 s here) on whichever request finds them due, and
/// the 24 searches take about 150 ms, so a window holds one such read or none
/// by where its clock stands. The two clusters run side by side with clocks
/// of their own, so one run saves a read (37 against 38 in CI, either way
/// round) and the next does not. A search that read one more blob per search
/// would add 24, so one is the noise and two is the failure.
fn search_costs_no_more(off: &[u64], on: &[u64]) -> bool {
    let at = |c: &[u64], op: S3Op| c[S3Op::ALL.iter().position(|o| *o == op).expect("op")];
    let others = |c: &[u64]| total(c) - at(c, S3Op::Get);
    others(on) <= others(off) && at(on, S3Op::Get) <= at(off, S3Op::Get) + 1
}

#[test]
fn small_indices_cost_no_more_requests_in_buckets_once_created() {
    // Two clusters of their own, each with its own store: side by side, the
    // test takes as long as one of them.
    let (off, on) = std::thread::scope(|s| {
        let off = s.spawn(|| measure(false));
        let on = measure(true);
        (off.join().expect("the buckets-off run"), on)
    });
    for (label, (a, b)) in [
        ("create", (&off.0, &on.0)),
        ("write", (&off.1, &on.1)),
        ("search", (&off.2, &on.2)),
        ("idle 8s", (&off.3, &on.3)),
    ] {
        show(&format!("{label} off"), a);
        show(&format!("{label} on "), b);
    }
    let at = |t: &[Duration], per_mille: usize| t[(t.len() * per_mille / 1000).min(t.len() - 1)];
    for (label, t) in [("off", &off.4), ("on ", &on.4)] {
        println!(
            "write latency {label}: p50 {:?} p99 {:?} ({} writes)",
            at(t, 500),
            at(t, 990),
            t.len()
        );
    }
    assert!(
        search_costs_no_more(&off.2, &on.2),
        "search: {off:?} {on:?}"
    );
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

/// The counts of a search window in `S3Op::ALL` order, from its plain reads
/// and ranged reads.
#[cfg(test)]
fn window(gets: u64, ranged: u64) -> Vec<u64> {
    S3Op::ALL
        .iter()
        .map(|op| match op {
            S3Op::Get => gets,
            S3Op::GetRange => ranged,
            _ => 0,
        })
        .collect()
}

#[test]
fn a_search_window_may_hold_one_more_secret_read_but_not_two_or_a_ranged_read() {
    // CI saw 37 plain reads without buckets and 38 with.
    assert!(search_costs_no_more(&window(37, 24), &window(38, 24)));
    assert!(search_costs_no_more(&window(38, 24), &window(37, 24)));
    assert!(search_costs_no_more(&window(38, 24), &window(38, 24)));
    assert!(!search_costs_no_more(&window(37, 24), &window(39, 24)));
    assert!(!search_costs_no_more(&window(38, 24), &window(38, 25)));
}

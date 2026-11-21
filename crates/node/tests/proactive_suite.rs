//! docs/specs/proactive-takeover-default: the takeover scenario of the
//! three-node suite with the proactive pass off and on, over the emulator
//! with latency or a RustFS process (`RUSTFS_BIN`): what a write and a search
//! cost after a node is killed. Evidence run by hand with `--ignored
//! --nocapture`; the gates are in proactive_faults and proactive_evidence.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

mod suite_rig;

use std::time::{Duration, Instant};

use oss_blob::S3Op;
use suite_rig::{Cluster, wait_for};

const TTL_MS: u64 = 3_000;
const SHARDS: u32 = 6;

fn one_doc(id: u32) -> String {
    format!("{{\"index\":{{\"_index\":\"logs\",\"_id\":\"{id}\"}}}}\n{{\"n\":{id}}}\n")
}

/// Indexes one document through `slot`, retrying until it is created; how
/// long that took.
fn write_one(cluster: &Cluster, slot: usize, id: u32) -> Duration {
    let body = one_doc(id);
    let started = Instant::now();
    loop {
        let r = cluster.call(slot, "POST", "/_bulk", Some(&body));
        if r.status == 200 && r.text().contains("\"errors\":false") {
            return started.elapsed();
        }
        assert!(started.elapsed() < Duration::from_secs(60), "{}", r.text());
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn search_ms(cluster: &Cluster, slot: usize) -> Duration {
    let started = Instant::now();
    let r = cluster.call(
        slot,
        "POST",
        "/logs/_search",
        Some(r#"{"query":{"match_all":{}},"size":10}"#),
    );
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(
        r.text().contains(&format!("\"successful\":{SHARDS}")),
        "{}",
        r.text()
    );
    started.elapsed()
}

struct Run {
    first_write_stall: Duration,
    writes: Vec<Duration>,
    searches: Vec<Duration>,
    requests: Option<u64>,
}

fn requests(cluster: &Cluster) -> u64 {
    [S3Op::Get, S3Op::GetRange, S3Op::Head, S3Op::Put, S3Op::List]
        .into_iter()
        .map(|op| cluster.sent(op))
        .sum()
}

/// Three nodes, 60 documents, the first killed; then 60 single-document
/// writes and 30 searches through the others.
fn run(mut cluster: Cluster, proactive: bool, emulated: bool) -> Run {
    cluster.set(
        "OSS_PROACTIVE_TAKEOVER",
        if proactive { "true" } else { "false" },
    );
    for id in ["n1", "n2", "n3"] {
        cluster.add(id);
    }
    wait_for("every node sees three", 30, || {
        (0..3).all(|s| cluster.nodes_seen(s) == 3)
    });
    std::thread::sleep(Duration::from_millis(TTL_MS));
    let body = format!("{{\"settings\":{{\"index\":{{\"number_of_shards\":{SHARDS}}}}}}}");
    assert_eq!(cluster.call(0, "PUT", "/logs", Some(&body)).status, 200);
    for id in 0..60 {
        write_one(&cluster, (id % 3) as usize, id);
    }
    let before = emulated.then(|| requests(&cluster));
    cluster.kill(0);
    wait_for("the others never saw the first node go", 30, || {
        cluster.nodes_seen(1) == 2 && cluster.nodes_seen(2) == 2
    });
    // Give a pass the time a node's refresh and a take need.
    if proactive {
        std::thread::sleep(Duration::from_secs(2));
    }
    let mut writes = Vec::new();
    for id in 100..160 {
        writes.push(write_one(&cluster, 1 + (id % 2) as usize, id));
    }
    let first_write_stall = writes.iter().copied().max().unwrap_or_default();
    let searches = (0..30).map(|n| search_ms(&cluster, 1 + (n % 2))).collect();
    Run {
        first_write_stall,
        writes,
        searches,
        requests: before.map(|b| requests(&cluster) - b),
    }
}

fn pct(all: &[Duration], q: f64) -> Duration {
    let mut all = all.to_vec();
    all.sort();
    all[(((all.len() - 1) as f64) * q) as usize]
}

fn report(label: &str, r: &Run) {
    eprintln!(
        "{label}: slowest write {:?}; writes p50 {:?} p99 {:?}; searches p50 {:?} p99 {:?}; requests since the kill {:?}",
        r.first_write_stall,
        pct(&r.writes, 0.5),
        pct(&r.writes, 0.99),
        pct(&r.searches, 0.5),
        pct(&r.searches, 0.99),
        r.requests
    );
}

#[test]
#[ignore = "evidence: run by hand with RUSTFS_BIN and --nocapture"]
fn the_takeover_scenario_with_the_pass_off_and_on_over_rustfs() {
    let Some(first) = Cluster::new(TTL_MS, 1, true) else {
        eprintln!("skipped: set RUSTFS_BIN to a RustFS binary to run this test");
        return;
    };
    let off = run(first, false, false);
    report("rustfs, pass off", &off);
    let on = run(Cluster::new(TTL_MS, 1, true).expect("rustfs"), true, false);
    report("rustfs, pass on", &on);
}

#[test]
#[ignore = "evidence: run by hand with --nocapture"]
fn the_takeover_scenario_with_the_pass_off_and_on_over_the_emulator() {
    let off = run(
        Cluster::new(TTL_MS, 1, false).expect("emulated"),
        false,
        true,
    );
    report("emulator, pass off", &off);
    let on = run(
        Cluster::new(TTL_MS, 1, false).expect("emulated"),
        true,
        true,
    );
    report("emulator, pass on", &on);
}

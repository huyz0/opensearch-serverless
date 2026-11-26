//! docs/specs/commit-cost-soak: what a node and a takeover ask of the store, and
//! what a write costs, with the commit switches off, with staging, and with
//! staging, logging and opening from logged commits.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: counts are small and a later one is never below an earlier one.
#![allow(clippy::arithmetic_side_effects)]

mod suite_rig;

use std::time::{Duration, Instant};

use oss_blob::S3Op;
use suite_rig::{Cluster, Reply, wait_for};

const INDICES: usize = 24;
/// The indices written continuously, one thread each, so a shard is
/// published several times between two checkpoints.
const HOT: usize = 6;
/// Writes each hot index takes (about a quarter second each).
const HOT_WRITES: usize = 40;

/// The checkpoint age the staged runs use: ten publishes of a hot shard at the
/// default publish age, well under the default minute.
const CHECKPOINT_MS: &str = "10000";

#[derive(Clone, Copy, Debug)]
enum Mode {
    Off,
    Staging,
    Logged,
}

fn counts(cluster: &Cluster) -> Vec<u64> {
    S3Op::ALL.iter().map(|op| cluster.sent(*op)).collect()
}

fn minus(now: &[u64], before: &[u64]) -> Vec<u64> {
    now.iter().zip(before).map(|(a, b)| a - b).collect()
}

fn total(c: &[u64]) -> u64 {
    c.iter().sum()
}

fn at(c: &[u64], op: S3Op) -> u64 {
    c[S3Op::ALL.iter().position(|o| *o == op).expect("op")]
}

fn show(label: &str, c: &[u64]) {
    let named: Vec<String> = S3Op::ALL
        .iter()
        .zip(c)
        .filter(|(_, n)| **n > 0)
        .map(|(op, n)| format!("{op:?}={n}"))
        .collect();
    println!("{label}: {} (total {})", named.join(" "), total(c));
}

fn set_mode(cluster: &mut Cluster, mode: Mode) {
    if matches!(mode, Mode::Staging | Mode::Logged) {
        cluster.set("OSS_COMMIT_STAGING", "true");
        cluster.set("OSS_COMMIT_CHECKPOINT_MS", CHECKPOINT_MS);
    }
    if matches!(mode, Mode::Logged) {
        cluster.set("OSS_COMMIT_LOGGED", "true");
        cluster.set("OSS_COMMIT_OPEN_FROM_LOGGED", "true");
    }
}

fn write_all(cluster: &Cluster, slot: usize, id: &str, refresh: bool) -> Vec<Duration> {
    let suffix = if refresh { "?refresh=true" } else { "" };
    let mut times = Vec::new();
    for i in 0..INDICES {
        let started = Instant::now();
        loop {
            let put = cluster.call(
                slot,
                "PUT",
                &format!("/small-{i}/_doc/{id}{suffix}"),
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
        times.push(started.elapsed());
    }
    times
}

fn search(cluster: &Cluster, slot: usize, target: &str) -> Reply {
    cluster.call(
        slot,
        "POST",
        &format!("{target}/_search"),
        Some(r#"{"query":{"match_all":{}}}"#),
    )
}

/// Every index answers with the documents written to it: one at create, one
/// before the kill, one after, and the hot indices' continuous writes.
fn keeps_documents(cluster: &Cluster, slot: usize) {
    for i in 0..INDICES {
        let docs = if i < HOT { HOT_WRITES + 3 } else { 3 };
        let target = format!("/small-{i}");
        let refreshed = cluster.call(slot, "POST", &format!("{target}/_refresh"), None);
        assert_eq!(refreshed.status, 200, "{}", refreshed.text());
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
        assert!(
            text.contains(&format!(r#""total":{{"value":{docs}"#)),
            "small-{i}: {text}"
        );
    }
}

struct Measured {
    /// Continuous writes to the hot indices, then 4 s idle.
    steady: Vec<u64>,
    times: Vec<Duration>,
    /// The second node writing to every index after the first was killed.
    takeover: Vec<u64>,
    takeover_took: Duration,
}

fn measure(mode: Mode) -> Measured {
    let mut cluster = Cluster::new(3_000, 50, false).expect("emulator");
    set_mode(&mut cluster, mode);
    cluster.add("n1");
    wait_for("the node sees itself", 30, || cluster.nodes_seen(0) == 1);
    for i in 0..INDICES {
        let made = cluster.call(0, "PUT", &format!("/small-{i}"), None);
        assert_eq!(made.status, 200, "{}", made.text());
    }
    write_all(&cluster, 0, "1", false);
    std::thread::sleep(Duration::from_millis(3_500));
    let start = counts(&cluster);
    // Continuous writes without refresh: the background pass publishes them.
    let mut times: Vec<Duration> = std::thread::scope(|scope| {
        let cluster = &cluster;
        let writers: Vec<_> = (0..HOT)
            .map(|i| {
                scope.spawn(move || {
                    let mut taken = Vec::new();
                    for n in 0..HOT_WRITES {
                        let started = Instant::now();
                        let put = cluster.call(
                            0,
                            "PUT",
                            &format!("/small-{i}/_doc/hot-{n}"),
                            Some(r#"{"n":1}"#),
                        );
                        assert_eq!(put.status, 201, "{}", put.text());
                        taken.push(started.elapsed());
                    }
                    taken
                })
            })
            .collect();
        writers
            .into_iter()
            .flat_map(|w| w.join().expect("a writer"))
            .collect()
    });
    std::thread::sleep(Duration::from_secs(4));
    let steady = minus(&counts(&cluster), &start);
    times.sort();
    // The last document each index took was acknowledged and may be only in the log.
    write_all(&cluster, 0, "last", false);
    cluster.add("n2");
    wait_for("both nodes see two", 30, || cluster.nodes_seen(1) == 2);
    cluster.kill(0);
    let before = counts(&cluster);
    let started = Instant::now();
    write_all(&cluster, 1, "after", false);
    let takeover_took = started.elapsed();
    let takeover = minus(&counts(&cluster), &before);
    keeps_documents(&cluster, 1);
    Measured {
        steady,
        times,
        takeover,
        takeover_took,
    }
}

#[test]
fn the_commit_switches_cost_no_more_than_the_defaults() {
    let (off, staging, logged) = std::thread::scope(|s| {
        let off = s.spawn(|| measure(Mode::Off));
        let staging = s.spawn(|| measure(Mode::Staging));
        let logged = measure(Mode::Logged);
        (
            off.join().expect("switches off"),
            staging.join().expect("staging"),
            logged,
        )
    });
    let p99 = |t: &[Duration]| t[(t.len() * 990 / 1000).min(t.len() - 1)];
    for (label, m) in [
        ("off    ", &off),
        ("staging", &staging),
        ("logged ", &logged),
    ] {
        show(&format!("{label} write+idle"), &m.steady);
        show(&format!("{label} takeover  "), &m.takeover);
        println!(
            "{label} write p50 {:?} p99 {:?}; takeover took {:?}",
            m.times[m.times.len() / 2],
            p99(&m.times),
            m.takeover_took
        );
    }
    // AC-1
    assert!(
        at(&staging.steady, S3Op::Put) < at(&off.steady, S3Op::Put),
        "{:?} {:?}",
        off.steady,
        staging.steady
    );
    assert!(
        total(&staging.steady) * 10 <= total(&off.steady) * 11 + 4,
        "{:?} {:?}",
        off.steady,
        staging.steady
    );
    // AC-2
    assert!(
        total(&logged.steady) * 10 <= total(&staging.steady) * 11 + 4,
        "{:?} {:?}",
        staging.steady,
        logged.steady
    );
    // AC-3
    for m in [&staging, &logged] {
        assert!(
            p99(&m.times).as_micros() * 100 <= p99(&off.times).as_micros() * 105,
            "{:?} {:?}",
            off.times,
            m.times
        );
    }
    // AC-4: the documents were checked inside `measure`. A takeover costs more
    // with staging (the successor replays what the register lacks and publishes
    // it); the bound pins how much, and opening from the logged commit costs
    // no more than staging alone.
    assert!(
        total(&staging.takeover) * 100 <= total(&off.takeover) * 115,
        "{:?} {:?}",
        off.takeover,
        staging.takeover
    );
    assert!(
        total(&logged.takeover) <= total(&staging.takeover),
        "{:?} {:?}",
        staging.takeover,
        logged.takeover
    );
}

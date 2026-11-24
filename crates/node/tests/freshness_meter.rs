//! docs/specs/node-freshness-meter: how long a write takes to be visible on
//! a node that does not own its shard.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: sample counts and millisecond figures are small.
#![allow(clippy::arithmetic_side_effects, clippy::cast_precision_loss)]

mod suite_rig;

use std::time::{Duration, Instant};

use suite_rig::{Cluster, wait_for};

/// A lease lifetime short enough to agree on owners in seconds.
const TTL_MS: u64 = 3_000;
/// Documents timed per series, after `WARMUP` that open the readers.
const SAMPLES: usize = 100;
const WARMUP: usize = 5;
/// The shell's `refresh=true` ack p99 on this latency profile (mode
/// `serving`, filesystem, 2026-11-12), and the tolerance on it.
const JAVA_P99_MS: f64 = 528.9;
const TOLERANCE: f64 = 1.05;

/// The nearest-rank percentile `p` (1 to 100) of `samples`: the smallest
/// sample at least `p` percent of them are not above.
fn percentile(samples: &[f64], p: usize) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = sorted.len().saturating_mul(p).div_ceil(100).max(1);
    sorted[rank.saturating_sub(1).min(sorted.len().saturating_sub(1))]
}

// AC-2
#[test]
fn the_percentile_is_the_nearest_rank() {
    let hundred: Vec<f64> = (1..=100).rev().map(f64::from).collect();
    assert_eq!(percentile(&hundred, 99), 99.0);
    assert_eq!(percentile(&hundred, 50), 50.0);
    assert_eq!(percentile(&hundred, 100), 100.0);
    assert_eq!(percentile(&hundred, 1), 1.0);
    assert_eq!(percentile(&[7.0], 99), 7.0);
    assert_eq!(percentile(&[3.0, 1.0, 2.0], 50), 2.0);
    assert_eq!(percentile(&[3.0, 1.0, 2.0, 4.0], 99), 4.0);
}

/// What one document cost.
struct Sample {
    ack: f64,
    visible: f64,
    misses: usize,
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Whether a search of `slot` counts document `id`.
fn counts(cluster: &Cluster, slot: usize, id: &str) -> bool {
    let body =
        format!(r#"{{"query":{{"ids":{{"values":["{id}"]}}}},"size":0,"track_total_hits":true}}"#);
    let r = cluster.call(slot, "POST", "/fresh/_search", Some(&body));
    let text = r.text();
    if std::env::var_os("FRESH_DEBUG").is_some() && !text.contains("\"value\":1") {
        eprintln!(
            "slot {slot} {id}: {} {}",
            r.status,
            &text[..text.len().min(300)]
        );
    }
    r.status == 200 && text.contains("\"value\":1")
}

/// Indexes `id` through node 0, then searches nodes 1 and 2 until both
/// count it.
fn timed(cluster: &Cluster, id: &str, refresh: bool) -> Sample {
    let target = if refresh {
        "/_bulk?refresh=true"
    } else {
        "/_bulk"
    };
    let body = format!("{{\"index\":{{\"_index\":\"fresh\",\"_id\":\"{id}\"}}}}\n{{\"n\":1}}\n");
    let start = Instant::now();
    let r = cluster.call(0, "POST", target, Some(&body));
    let ack = ms(start);
    assert!(
        r.status == 200 && r.text().contains("\"errors\":false"),
        "{}",
        r.text()
    );
    let mut seen = [None, None];
    let mut misses = 0;
    let mut first = true;
    while seen.iter().any(Option::is_none) {
        assert!(start.elapsed() < Duration::from_secs(10), "{id} never seen");
        for (at, slot) in [1, 2].into_iter().enumerate() {
            if seen[at].is_none() {
                if counts(cluster, slot, id) {
                    seen[at] = Some(ms(start));
                } else if first {
                    misses += 1;
                }
            }
        }
        first = false;
    }
    let visible = seen.iter().flatten().copied().fold(0.0, f64::max);
    Sample {
        ack,
        visible,
        misses,
    }
}

fn series(cluster: &Cluster, refresh: bool) -> (Vec<Sample>, Vec<(oss_blob::S3Op, u64)>) {
    let label = if refresh { "r" } else { "w" };
    for n in 0..WARMUP {
        timed(cluster, &format!("{label}warm{n}"), refresh);
    }
    let before = oss_blob::S3Op::ALL.map(|op| cluster.sent(op));
    let samples: Vec<Sample> = (0..SAMPLES)
        .map(|n| timed(cluster, &format!("{label}{n}"), refresh))
        .collect();
    let sent = oss_blob::S3Op::ALL
        .iter()
        .zip(before)
        .map(|(op, b)| (*op, cluster.sent(*op).saturating_sub(b)))
        .filter(|(_, n)| *n > 0)
        .collect();
    (samples, sent)
}

fn report(refresh: bool, samples: &[Sample], sent: &[(oss_blob::S3Op, u64)]) -> (f64, usize) {
    let col = |f: fn(&Sample) -> f64| samples.iter().map(f).collect::<Vec<_>>();
    let (ack, visible) = (col(|s| s.ack), col(|s| s.visible));
    let misses: usize = samples.iter().map(|s| s.misses).sum();
    let per_write: Vec<String> = sent
        .iter()
        .map(|(op, n)| format!("{op:?} {:.3}", *n as f64 / samples.len() as f64))
        .collect();
    println!(
        "FRESHMETER|refresh={refresh}|ack p50={:.1} p99={:.1} ms|visible p50={:.1} p99={:.1} max={:.1} ms (n={})|misses {misses} of {}|per write: {}|provisional: emulator",
        percentile(&ack, 50),
        percentile(&ack, 99),
        percentile(&visible, 50),
        percentile(&visible, 99),
        percentile(&visible, 100),
        samples.len(),
        samples.len() * 2,
        per_write.join(" ")
    );
    (percentile(&visible, 99), misses)
}

// AC-1, AC-3, AC-4
#[test]
#[ignore = "slow"]
fn a_write_is_visible_on_the_other_nodes_within_the_shells_bar() {
    let mut cluster = Cluster::new(TTL_MS, 1, false).expect("emulated");
    // `METER_TAIL`: the refresh visibility (`owner`, `serving` or
    // `everywhere`, the default) the overlay is on under.
    let tail = std::env::var("METER_TAIL").unwrap_or_else(|_| "everywhere".to_owned());
    println!("FRESHMETER|visibility={tail}");
    // `METER_OVERLAY`: `false` runs with the tail overlay off, where a
    // refresh publishes a commit.
    let overlay = std::env::var("METER_OVERLAY").map_or(true, |v| v != "false");
    println!("FRESHMETER|overlay={overlay}");
    cluster.set("OSS_TAIL_OVERLAY", if overlay { "true" } else { "false" });
    cluster.set("OSS_REFRESH_VISIBILITY", &tail);
    // `METER_REFRESH_MS`: how often each node reads the fleet's leases;
    // unset, a third of the lease lifetime.
    if let Ok(ms) = std::env::var("METER_REFRESH_MS") {
        println!("FRESHMETER|lease refresh={ms} ms");
        cluster.set("OSS_LEASE_REFRESH_MS", &ms);
    }
    // `METER_PUSH`: `false` turns the commit push off.
    if let Ok(push) = std::env::var("METER_PUSH") {
        println!("FRESHMETER|commit push={push}");
        cluster.set("OSS_COMMIT_PUSH", &push);
    }
    for id in ["n1", "n2", "n3"] {
        cluster.add(id);
    }
    wait_for("every node sees three", 30, || {
        (0..3).all(|s| cluster.nodes_seen(s) == 3)
    });
    std::thread::sleep(Duration::from_millis(TTL_MS));
    assert_eq!(
        (0..3).all(|s| cluster.node(s).tail_on()),
        overlay,
        "overlay"
    );
    let made = cluster.call(
        0,
        "PUT",
        "/fresh",
        Some(r#"{"settings":{"index":{"number_of_shards":1}},"mappings":{"properties":{"n":{"type":"integer"}}}}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    let bar = JAVA_P99_MS * TOLERANCE;
    for refresh in [false, true] {
        let (samples, sent) = series(&cluster, refresh);
        let (p99, misses) = report(refresh, &samples, &sent);
        // A write that waits for its readers is never missed by them.
        assert!(
            !(overlay && refresh && tail != "owner") || misses == 0,
            "refresh=true under {tail}: {misses} misses"
        );
        assert!(
            p99 <= bar,
            "refresh={refresh}: visible p99 {p99:.1} ms > {bar:.1}"
        );
    }
}

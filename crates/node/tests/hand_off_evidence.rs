//! docs/specs/writer-hand-off: what turning hand-off and steering on costs
//! a fleet that gains an empty node. Two nodes are filled to their cap, a
//! third joins, and a new index is written to; the run reports the write
//! latency, the requests and the share of shards the new node ends up with,
//! with the switch off and on, over the emulator with latency. Evidence run
//! by hand with `--ignored --nocapture`; a percentile is not a predicate.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

mod suite_rig;

use std::time::{Duration, Instant};

use oss_blob::S3Op;
use suite_rig::{Cluster, wait_for};

const TTL_MS: u64 = 3_000;
const SHARDS: u32 = 6;
const CAP: &str = "12";
const OLD: [&str; 4] = ["cold1", "cold2", "cold3", "cold4"];
const NEW: [&str; 4] = ["new1", "new2", "new3", "new4"];

fn bulk_body(index: &str, ids: std::ops::Range<u32>) -> String {
    let mut body = String::new();
    for id in ids {
        body.push_str(&format!(
            "{{\"index\":{{\"_index\":\"{index}\",\"_id\":\"{id}\"}}}}\n{{\"n\":{id}}}\n"
        ));
    }
    body
}

fn create(cluster: &Cluster, slot: usize, index: &str) {
    let body = format!("{{\"settings\":{{\"index\":{{\"number_of_shards\":{SHARDS}}}}}}}");
    let made = cluster.call(slot, "PUT", &format!("/{index}"), Some(&body));
    assert_eq!(made.status, 200, "{}", made.text());
}

/// A bulk of 60 documents (every shard of the index) through `slot`, retried
/// until created; how long that took and how many tries.
fn bulk(cluster: &Cluster, slot: usize, index: &str, round: u32) -> (Duration, u32) {
    let body = bulk_body(index, round * 60..round * 60 + 60);
    let started = Instant::now();
    let mut tries = 0;
    loop {
        tries += 1;
        let r = cluster.call(slot, "POST", "/_bulk?refresh=true", Some(&body));
        if r.status == 200 && r.text().contains("\"errors\":false") {
            return (started.elapsed(), tries);
        }
        if tries <= 3 {
            eprintln!(
                "retry {index} {round}: {} {}",
                r.status,
                &r.text()[..r.text().len().min(300)]
            );
        }
        assert!(
            started.elapsed().as_secs() < 60,
            "never created: {}",
            r.text()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let at = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[at]
}

fn requests(cluster: &Cluster) -> u64 {
    [
        S3Op::Get,
        S3Op::GetRange,
        S3Op::Head,
        S3Op::Put,
        S3Op::List,
        S3Op::Delete,
    ]
    .into_iter()
    .map(|op| cluster.sent(op))
    .sum()
}

/// How long each cold index takes to answer a match_all through node 3,
/// retried while a shard refuses; each must find every document.
fn cold_searches(cluster: &Cluster) -> Vec<Duration> {
    let mut searches = Vec::new();
    for name in OLD {
        let started = Instant::now();
        let mut total = 0;
        wait_for("a search answers every shard", 30, || {
            let r = cluster.call(
                2,
                "POST",
                &format!("/{name}/_search"),
                Some(r#"{"query":{"match_all":{}},"size":0,"track_total_hits":true}"#),
            );
            let text = r.text();
            if r.status != 200 || !text.contains(&format!("\"successful\":{SHARDS}")) {
                eprintln!(
                    "search {name}: {} {}",
                    r.status,
                    &text[..text.len().min(300)]
                );
                return false;
            }
            let (_, rest) = text.split_once("\"value\":").expect("a total");
            total = rest
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u64>()
                .expect("a number");
            true
        });
        assert_eq!(total, 60, "{name}");
        searches.push(started.elapsed());
    }
    searches
}

fn run(hand_off: bool) {
    let mut cluster = Cluster::new(TTL_MS, 4, false).expect("emulated");
    cluster.set(
        "OSS_MAX_SHARDS",
        &std::env::var("EVIDENCE_CAP").unwrap_or_else(|_| CAP.to_owned()),
    );
    cluster.set("OSS_EVICT_AFTER_MS", "2000");
    cluster.set("OSS_HAND_OFF", if hand_off { "true" } else { "false" });
    for id in ["n1", "n2"] {
        cluster.add(id);
    }
    wait_for("both see two", 30, || {
        (0..2).all(|s| cluster.nodes_seen(s) == 2)
    });
    std::thread::sleep(Duration::from_millis(TTL_MS));
    // Two nodes of cap 12 hold 24 shards: nearly full each.
    for name in OLD {
        create(&cluster, 0, name);
        bulk(&cluster, 0, name, 0);
    }
    cluster.add("n3");
    wait_for("all see three", 30, || {
        (0..3).all(|s| cluster.nodes_seen(s) == 3)
    });
    eprintln!(
        "cold searches before any eviction: {:?}",
        cold_searches(&cluster)
    );
    // Let the fleet's leases carry the load and the shards go idle.
    std::thread::sleep(Duration::from_millis(TTL_MS * 2));
    let held = |c: &Cluster| (0..3).map(|s| c.held_of(s)).collect::<Vec<_>>();
    eprintln!("held before the new indexes: {:?}", held(&cluster));
    let before = requests(&cluster);
    let mut times = Vec::new();
    let mut retries = 0;
    for name in NEW {
        create(&cluster, 0, name);
        for round in 0..10 {
            let (t, tries) = bulk(&cluster, 0, name, round);
            times.push(t);
            retries += tries - 1;
            std::thread::sleep(Duration::from_millis(150));
        }
    }
    std::thread::sleep(Duration::from_millis(TTL_MS));
    eprintln!("held after: {:?}", held(&cluster));
    let searches = cold_searches(&cluster);
    eprintln!("cold searches: {searches:?}");
    times.sort();
    let spent = requests(&cluster) - before;
    let mut on_n3 = 0;
    let mut owned = 0;
    for name in NEW {
        for shard in 0..SHARDS {
            if let Some(slot) = cluster.owner_slot(name, shard) {
                owned += 1;
                on_n3 += usize::from(slot == 2);
            }
        }
    }
    eprintln!(
        "RESULT hand_off={hand_off} bulks={} p50={:?} p99={:?} max={:?} retries={retries} requests={spent} new_shards_on_n3={on_n3}/{owned}",
        times.len(),
        percentile(&times, 0.5),
        percentile(&times, 0.99),
        times.last().expect("a bulk"),
    );
}

#[test]
#[ignore = "evidence: run by hand with --nocapture"]
fn a_fleet_that_gains_an_empty_node_with_hand_off_off() {
    run(false);
}

#[test]
#[ignore = "evidence: run by hand with --nocapture"]
fn a_fleet_that_gains_an_empty_node_with_hand_off_on() {
    run(true);
}

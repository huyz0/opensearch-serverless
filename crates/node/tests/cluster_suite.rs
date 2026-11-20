//! docs/specs/node-cluster-suite: three real nodes on one store behind an
//! S3 front end with request latency (and on RustFS when `RUSTFS_BIN` is
//! set), through REST only.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

mod suite_rig;

use std::collections::BTreeSet;
use std::time::Duration;

use suite_rig::{Cluster, wait_for};

/// A lease lifetime short enough that a killed node is seen to go in seconds.
const TTL_MS: u64 = 3_000;
const SHARDS: u32 = 6;

/// A bulk body indexing `ids` of `index`.
fn bulk_body(index: &str, ids: std::ops::Range<u32>) -> String {
    let mut body = String::new();
    for id in ids {
        body.push_str(&format!(
            "{{\"index\":{{\"_index\":\"{index}\",\"_id\":\"{id}\"}}}}\n{{\"n\":{id}}}\n"
        ));
    }
    body
}

/// Three started nodes that each see the other two, and have had a lease
/// lifetime to agree on who owns what.
fn started(mut cluster: Cluster) -> Cluster {
    for id in ["n1", "n2", "n3"] {
        cluster.add(id);
    }
    wait_for("every node sees three", 30, || {
        (0..3).all(|s| cluster.nodes_seen(s) == 3)
    });
    std::thread::sleep(Duration::from_millis(TTL_MS));
    cluster
}

fn create(cluster: &Cluster, slot: usize, index: &str) {
    let body = format!("{{\"settings\":{{\"index\":{{\"number_of_shards\":{SHARDS}}}}}}}");
    let made = cluster.call(slot, "PUT", &format!("/{index}"), Some(&body));
    assert_eq!(made.status, 200, "{}", made.text());
}

/// Indexes `ids` through the node in `slot` with `refresh=true`, until
/// every item is created or 60 s pass: a node whose view of the members is
/// a renewal behind may name the wrong owner, and the shard head refuses it.
fn bulk(cluster: &Cluster, slot: usize, index: &str, ids: std::ops::Range<u32>) {
    let body = bulk_body(index, ids);
    let started = std::time::Instant::now();
    loop {
        let r = cluster.call(slot, "POST", "/_bulk?refresh=true", Some(&body));
        let text = r.text();
        if r.status == 200 && text.contains("\"errors\":false") {
            return;
        }
        assert!(
            started.elapsed().as_secs() < 60,
            "the bulk never succeeded: {text}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The slots of the live nodes that hold `id` (a node that does not hold
/// the shard answers 503: reads are not forwarded).
fn holders(cluster: &Cluster, index: &str, id: u32) -> Vec<usize> {
    cluster
        .live()
        .into_iter()
        .filter(|slot| {
            let r = cluster.call(*slot, "GET", &format!("/{index}/_doc/{id}"), None);
            r.status == 200 && r.text().contains("\"found\":true")
        })
        .collect()
}

/// A search from `slot` that every shard answered, retried while one did
/// not (#217, #218).
fn search_total(cluster: &Cluster, slot: usize, index: &str) -> u64 {
    let mut total = 0;
    wait_for("no search answered from every shard", 60, || {
        let r = cluster.call(
            slot,
            "POST",
            &format!("/{index}/_search"),
            Some(r#"{"query":{"match_all":{}},"size":0,"track_total_hits":true}"#),
        );
        let text = r.text();
        if r.status != 200 || !text.contains(&format!("\"successful\":{SHARDS}")) {
            eprintln!(
                "search via {slot}: {} {}",
                r.status,
                &text[..text.len().min(700)]
            );
            return false;
        }
        let (_, rest) = text.split_once("\"value\":").expect("a total");
        total = rest
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .expect("a number");
        true
    });
    total
}

// AC-1 and AC-2
#[test]
fn a_bulk_through_any_node_lands_on_the_node_that_owns_each_shard() {
    let cluster = started(Cluster::new(TTL_MS, 4, false).expect("emulated"));
    create(&cluster, 0, "logs");
    for (slot, ids) in [(0, 0..20), (1, 20..40), (2, 40..60)] {
        bulk(&cluster, slot, "logs", ids);
    }
    let mut serving = BTreeSet::new();
    for id in 0..60 {
        let held = holders(&cluster, "logs", id);
        assert_eq!(held.len(), 1, "doc {id} is on {held:?}");
        serving.extend(held);
    }
    assert!(
        serving.len() >= 2,
        "one node holds every shard: {serving:?}"
    );
}

// AC-3
#[test]
#[ignore = "gap: issues #217 (no I/O buffer free under a fan-out) and #218 (a shard's published commit has no segments_N)"]
fn a_search_from_any_node_finds_every_document() {
    let cluster = started(Cluster::new(TTL_MS, 4, false).expect("emulated"));
    create(&cluster, 2, "logs");
    for (slot, ids) in [(2, 0..15), (0, 15..30), (1, 30..45)] {
        bulk(&cluster, slot, "logs", ids);
    }
    for slot in 0..3 {
        assert_eq!(search_total(&cluster, slot, "logs"), 45, "via node {slot}");
    }
}

/// Writes, kills the first node, and checks the others take its shards
/// with everything it wrote.
fn takeover(cluster: Cluster) {
    let mut cluster = started(cluster);
    create(&cluster, 0, "logs");
    for (slot, ids) in [(0, 0..20), (1, 20..40), (2, 40..60)] {
        bulk(&cluster, slot, "logs", ids);
    }
    cluster.kill(0);
    // Its lease is not released: the others drop it when it lapses.
    wait_for("the others never saw the first node go", 30, || {
        cluster.nodes_seen(1) == 2 && cluster.nodes_seen(2) == 2
    });
    // A write to every shard makes the new owner take it over.
    bulk(&cluster, 1, "logs", 100..160);
    bulk(&cluster, 2, "logs", 160..180);
    for id in (0..60).chain(100..180) {
        let held = holders(&cluster, "logs", id);
        assert_eq!(held.len(), 1, "doc {id} is on {held:?}");
    }
}

// AC-4
#[test]
fn the_others_take_over_a_killed_nodes_shards_with_its_documents() {
    takeover(Cluster::new(TTL_MS, 4, false).expect("emulated"));
}

// AC-5
#[test]
fn the_same_scenario_runs_on_rustfs() {
    let Some(cluster) = Cluster::new(TTL_MS, 1, true) else {
        eprintln!("skipped: set RUSTFS_BIN to a RustFS binary to run this test");
        return;
    };
    takeover(cluster);
}

//! docs/specs/reader-overlay-dynamic: an index with no mapping is served
//! by the overlay on another node.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: sample counts and request counts are small.
#![allow(clippy::arithmetic_side_effects, clippy::cast_precision_loss)]

mod suite_rig;

use std::time::{Duration, Instant};

use oss_blob::S3Op;
use suite_rig::{Cluster, wait_for};

const TTL_MS: u64 = 3_000;
const WRITES: usize = 20;

fn write(cluster: &Cluster, index: &str, id: &str) {
    let body = format!("{{\"index\":{{\"_index\":\"{index}\",\"_id\":\"{id}\"}}}}\n{{\"n\":1}}\n");
    let r = cluster.call(0, "POST", "/_bulk", Some(&body));
    assert!(
        r.status == 200 && r.text().contains("\"errors\":false"),
        "{}",
        r.text()
    );
}

fn counts(cluster: &Cluster, slot: usize, index: &str, id: &str) -> bool {
    let body =
        format!(r#"{{"query":{{"ids":{{"values":["{id}"]}}}},"size":0,"track_total_hits":true}}"#);
    let r = cluster.call(slot, "POST", &format!("/{index}/_search"), Some(&body));
    r.status == 200 && r.text().contains("\"value\":1")
}

fn requests(cluster: &Cluster) -> u64 {
    S3Op::ALL.iter().map(|op| cluster.sent(*op)).sum()
}

/// Writes `WRITES` documents and returns the requests they cost, having
/// waited for each to be counted on nodes 1 and 2 within `within`.
fn run(cluster: &Cluster, index: &str, within: Duration) -> u64 {
    for n in 0..3 {
        write(cluster, index, &format!("warm{n}"));
    }
    // Until the first commit is published a reader has no shard to open.
    for slot in [1, 2] {
        wait_for("the warm-up documents reach the readers", 40, || {
            counts(cluster, slot, index, "warm2")
        });
    }
    let before = requests(cluster);
    for n in 0..WRITES {
        let id = format!("d{n}");
        let start = Instant::now();
        write(cluster, index, &id);
        for slot in [1, 2] {
            while !counts(cluster, slot, index, &id) {
                assert!(
                    start.elapsed() < within,
                    "{index}: {id} not counted on node {slot} within {within:?}"
                );
            }
        }
    }
    requests(cluster) - before
}

// AC-4 and AC-5
#[test]
#[ignore = "slow"]
fn an_index_without_a_mapping_is_served_by_the_overlay_at_the_cost_of_one_with() {
    let mut cluster = Cluster::new(TTL_MS, 4, false).expect("emulated");
    cluster.set("OSS_TAIL_OVERLAY", "true");
    for id in ["n1", "n2", "n3"] {
        cluster.add(id);
    }
    wait_for("every node sees three", 30, || {
        (0..3).all(|s| cluster.nodes_seen(s) == 3)
    });
    std::thread::sleep(Duration::from_millis(TTL_MS));
    let settings = r#""settings":{"index":{"number_of_shards":1}}"#;
    for (name, mappings) in [
        (
            "mapped",
            r#","mappings":{"properties":{"n":{"type":"integer"}}}"#,
        ),
        ("dynamic", ""),
    ] {
        let made = cluster.call(
            0,
            "PUT",
            &format!("/{name}"),
            Some(&format!("{{{settings}{mappings}}}")),
        );
        assert_eq!(made.status, 200, "{}", made.text());
    }
    // The first documents open the readers and reach the first commit; the
    // timed ones must then be served before the next publish (10 s).
    let mapped = run(&cluster, "mapped", Duration::from_secs(3));
    let dynamic = run(&cluster, "dynamic", Duration::from_secs(3));
    println!("OVERLAYDYNAMIC|requests for {WRITES} writes: mapped {mapped}, dynamic {dynamic}");
    // A refreshing write to the unmapped index is counted by the first
    // search of each other node, as the shell promises.
    let body = "{\"index\":{\"_index\":\"dynamic\",\"_id\":\"fresh\"}}\n{\"n\":1}\n";
    let r = cluster.call(0, "POST", "/_bulk?refresh=true", Some(body));
    assert_eq!(r.status, 200, "{}", r.text());
    for slot in [1, 2] {
        assert!(counts(&cluster, slot, "dynamic", "fresh"), "node {slot}");
    }
    assert!(
        dynamic as f64 <= mapped as f64 * 1.1,
        "dynamic {dynamic} > 110% of mapped {mapped}"
    );
}

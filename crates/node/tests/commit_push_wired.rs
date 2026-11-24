//! docs/specs/node-commit-push: a published commit reaches the readers that
//! hold the shard open, in a booted cluster.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

mod suite_rig;

use std::time::{Duration, Instant};

use suite_rig::{Cluster, wait_for};

const TTL_MS: u64 = 3_000;

fn started(push: Option<&str>) -> Cluster {
    let mut cluster = Cluster::new(TTL_MS, 4, false).expect("emulated");
    if let Some(push) = push {
        cluster.set("OSS_COMMIT_PUSH", push);
    }
    for id in ["n1", "n2", "n3"] {
        cluster.add(id);
    }
    wait_for("every node sees three", 30, || {
        (0..3).all(|s| cluster.nodes_seen(s) == 3)
    });
    std::thread::sleep(Duration::from_millis(TTL_MS));
    let made = cluster.call(
        0,
        "PUT",
        "/logs",
        Some(r#"{"settings":{"index":{"number_of_shards":1}},"mappings":{"properties":{"n":{"type":"integer"}}}}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    cluster
}

fn write(cluster: &Cluster, id: &str) {
    let body = format!("{{\"index\":{{\"_index\":\"logs\",\"_id\":\"{id}\"}}}}\n{{\"n\":1}}\n");
    let r = cluster.call(0, "POST", "/_bulk?refresh=true", Some(&body));
    assert!(
        r.status == 200 && r.text().contains("\"errors\":false"),
        "{}",
        r.text()
    );
}

fn total(cluster: &Cluster, slot: usize) -> u64 {
    let r = cluster.call(
        slot,
        "POST",
        "/logs/_search",
        Some(r#"{"query":{"match_all":{}},"size":0,"track_total_hits":true}"#),
    );
    r.text()
        .split_once("\"value\":")
        .and_then(|(_, rest)| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
        .unwrap_or(0)
}

/// Writes `a`, opens the shard on every node by a search, writes `b` and
/// refreshes it on its owner.
fn second_write(cluster: &Cluster) {
    write(cluster, "a");
    let owner = cluster.owner_slot("logs", 0).expect("an owner");
    for slot in 0..3 {
        assert_eq!(total(cluster, slot), 1, "first search on node {slot}");
    }
    write(cluster, "b");
    let r = cluster.call(owner, "POST", "/logs/_refresh", None);
    assert_eq!(r.status, 200, "{}", r.text());
}

// AC-1 and AC-2
#[test]
fn a_commit_the_owner_publishes_reaches_every_node_that_has_the_shard_open() {
    let cluster = started(None);
    second_write(&cluster);
    for slot in 0..3 {
        let started = Instant::now();
        while total(&cluster, slot) != 2 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "node {slot} never counted the second write"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

// AC-3
#[test]
fn with_the_setting_off_an_opened_shard_stays_on_its_commit() {
    let cluster = started(Some("false"));
    second_write(&cluster);
    std::thread::sleep(Duration::from_secs(4));
    // The owner's own reader takes its writer's commit whatever the setting
    // says; what it turns off is the send to the other nodes.
    let owner = cluster.owner_slot("logs", 0).expect("an owner");
    for slot in (0..3).filter(|s| *s != owner) {
        assert_eq!(total(&cluster, slot), 1, "node {slot} moved on");
    }
}

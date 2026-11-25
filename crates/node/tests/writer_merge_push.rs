//! docs/specs/writer-segment-merge AC-10: readers that hold a shard open
//! follow the owner's pushed commits across merges, and a node that opens it
//! late reads the merged commit.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

mod suite_rig;

use std::time::{Duration, Instant};

use suite_rig::{Cluster, wait_for};

const TTL_MS: u64 = 3_000;
const WRITES: usize = 14;

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

fn counts(cluster: &Cluster, slot: usize, want: u64) {
    let started = Instant::now();
    while total(cluster, slot) != want {
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "node {slot} counts {} of {want}",
            total(cluster, slot)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

// docs/specs/writer-segment-merge AC-10
#[test]
fn readers_follow_pushed_commits_across_merges_and_a_late_node_reads_the_merged_commit() {
    let mut cluster = Cluster::new(TTL_MS, 4, false).expect("emulated");
    cluster.set("OSS_TAIL_OVERLAY", "false");
    cluster.set("OSS_WRITER_MERGE", "true");
    cluster.set("OSS_WRITER_MERGE_SEGMENTS_PER_TIER", "2");
    cluster.set("OSS_WRITER_MERGE_AT_ONCE", "4");
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
    let owner = (0..1)
        .find_map(|_| {
            let w = cluster.call(
                0,
                "POST",
                "/_bulk?refresh=true",
                Some("{\"index\":{\"_index\":\"logs\",\"_id\":\"first\"}}\n{\"n\":1}\n"),
            );
            assert_eq!(w.status, 200, "{}", w.text());
            cluster.owner_slot("logs", 0)
        })
        .expect("an owner");
    // A view taken now freezes this commit, before any merge.
    let pit = cluster.call(
        0,
        "POST",
        "/logs/_search/point_in_time?keep_alive=30m",
        None,
    );
    assert_eq!(pit.status, 200, "{}", pit.text());
    let pit_id = pit
        .text()
        .split_once("\"pit_id\":\"")
        .and_then(|(_, rest)| rest.split_once('"').map(|(id, _)| id.to_owned()))
        .expect("a pit id");
    // Every node opens the shard now, so each later commit is pushed to it.
    for slot in 0..3 {
        counts(&cluster, slot, 1);
    }
    for i in 0..WRITES {
        let body = format!("{{\"index\":{{\"_index\":\"logs\",\"_id\":\"d{i}\"}}}}\n{{\"n\":1}}\n");
        let w = cluster.call(0, "POST", "/_bulk?refresh=true", Some(&body));
        assert_eq!(w.status, 200, "{}", w.text());
        let r = cluster.call(owner, "POST", "/logs/_refresh", None);
        assert_eq!(r.status, 200, "{}", r.text());
        for slot in 0..3 {
            counts(&cluster, slot, i as u64 + 2);
        }
    }
    // The view still reads the commit it froze, whose segments were merged away.
    let frozen = format!(
        r#"{{"pit":{{"id":"{pit_id}"}},"query":{{"match_all":{{}}}},"size":0,"track_total_hits":true}}"#
    );
    for slot in 0..3 {
        let r = cluster.call(slot, "POST", "/_search", Some(&frozen));
        assert_eq!(r.status, 200, "node {slot}: {}", r.text());
        assert!(
            r.text().contains(r#""total":{"value":1,"relation":"eq"}"#),
            "node {slot}: {}",
            r.text()
        );
    }
    // The published commit holds merged segments.
    let (endpoint, bucket, region, access, secret) = cluster.store();
    let store = oss_blob::S3Store::new(oss_blob::S3Config::new(
        &endpoint,
        bucket,
        region,
        oss_blob::Credentials::new(access, secret),
    ))
    .expect("a store");
    let pools = oss_mem::BudgetConfig::new(8 << 20)
        .with_pool(oss_mem::Pool::Transport, 8 << 20)
        .expect("fits");
    let buffers = oss_mem::BufferPool::new(
        &oss_mem::MemoryBudget::new(&pools),
        oss_mem::Pool::Transport,
        64 << 10,
        16,
    )
    .expect("fits");
    let pooled = std::sync::Arc::new(oss_blob::PooledStore::new(store, buffers));
    let cache = std::sync::Arc::new(
        lucene_store::page_cache::PageCache::new(
            lucene_store::page_cache::PageCacheConfig::default(),
        )
        .expect("cache"),
    );
    let open = oss_reader::open_shard(pooled, cache, "", "logs", 0).expect("opens");
    let segments = open.reader.segment_readers().len();
    assert!(
        segments < WRITES / 2,
        "{segments} segments after {WRITES} commits"
    );
    // A node that joins now opens the shard from the merged commit.
    let late = cluster.add("n4");
    wait_for("the late node sees four", 30, || {
        cluster.nodes_seen(late) == 4
    });
    counts(&cluster, late, WRITES as u64 + 1);
}

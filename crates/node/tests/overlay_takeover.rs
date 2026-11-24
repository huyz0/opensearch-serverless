//! docs/specs/overlay-takeover-faults: a writer killed while readers hold its
//! tail is taken over without an acknowledged write lost, doubled or stale,
//! and the overlay is rebuilt against the successor.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

mod suite_rig;

use std::time::{Duration, Instant};

use suite_rig::{Cluster, wait_for};

const TTL_MS: u64 = 3_000;

/// One cluster at a time: leases last three seconds, and four clusters of
/// three nodes on one machine starve each other's renewals.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn alone() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn started() -> Cluster {
    let mut cluster = Cluster::new(TTL_MS, 4, false).expect("emulated");
    cluster.set("OSS_TAIL_OVERLAY", "true");
    cluster.set("OSS_REFRESH_VISIBILITY", "everywhere");
    for id in ["n1", "n2", "n3"] {
        cluster.add(id);
    }
    wait_for("every node sees three", 30, || {
        (0..3).all(|s| cluster.nodes_seen(s) == 3)
    });
    std::thread::sleep(Duration::from_millis(TTL_MS));
    assert!((0..3).all(|s| cluster.node(s).tail_on()), "overlay on");
    let made = cluster.call(
        0,
        "PUT",
        "/logs",
        Some(r#"{"settings":{"index":{"number_of_shards":1}},"mappings":{"properties":{"n":{"type":"integer"}}}}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    cluster
}

/// Indexes `ids` with `refresh=true` until every item is created or 60 s
/// pass: a write refused during the takeover is not acknowledged, and indexing
/// by id again is the same write.
fn bulk(cluster: &Cluster, slot: usize, ids: &[String], n: u32) {
    let body: String = ids
        .iter()
        .map(|id| {
            format!("{{\"index\":{{\"_index\":\"logs\",\"_id\":\"{id}\"}}}}\n{{\"n\":{n}}}\n")
        })
        .collect();
    let started = Instant::now();
    loop {
        let r = cluster.call(slot, "POST", "/_bulk?refresh=true", Some(&body));
        if r.status == 200 && r.text().contains("\"errors\":false") {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the bulk never succeeded: {}",
            r.text()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn ids(range: std::ops::Range<u32>) -> Vec<String> {
    range.map(|i| format!("d{i}")).collect()
}

/// What a search of `slot` for documents whose `n` is `value` answers: the
/// hit total and how many shards failed.
fn count(cluster: &Cluster, slot: usize, value: Option<u32>) -> Option<(u64, u64)> {
    let query = value.map_or_else(
        || r#"{"match_all":{}}"#.to_owned(),
        |v| format!(r#"{{"term":{{"n":{v}}}}}"#),
    );
    let body = format!(r#"{{"query":{query},"size":0,"track_total_hits":true}}"#);
    let r = cluster.call(slot, "POST", "/logs/_search", Some(&body));
    if r.status != 200 {
        return None;
    }
    let text = r.text();
    let number = |after: &str| -> u64 {
        text.split_once(after)
            .map(|(_, rest)| {
                rest.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
            })
            .and_then(|digits| digits.parse().ok())
            .unwrap_or(0)
    };
    Some((number("\"value\":"), number("\"failed\":")))
}

/// [`count`], asked again while the search is refused: a copy that is not
/// yet caught up reports the shard failed (503) rather than a stale count,
/// and must answer within 10 s.
fn answered(cluster: &Cluster, slot: usize, value: Option<u32>) -> (u64, u64) {
    let started = Instant::now();
    loop {
        if let Some(answer) = count(cluster, slot, value) {
            return answer;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "node {slot} kept refusing the search"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Polls `slot` until `value`'s count is `want`, failing if any poll counts
/// more than `want` (a document seen twice) or a poll that reports no shard
/// failure counts fewer once `settled` (an acknowledged write lost).
fn converges(cluster: &Cluster, slot: usize, value: Option<u32>, want: u64) {
    let started = Instant::now();
    let mut last = None;
    loop {
        last = count(cluster, slot, value).or(last);
        if let Some((total, failed)) = count(cluster, slot, value) {
            assert!(
                total <= want,
                "node {slot} counted {total}, more than {want}"
            );
            if total == want && failed == 0 {
                return;
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "node {slot} never counted {want} for {value:?}; last answer {last:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Writes through the owner's peers, waits for both readers to hold the
/// tail, kills the owner and waits for the others to see it go.
fn killed_owner() -> (Cluster, usize) {
    let mut cluster = started();
    // 20 documents, then five of them updated: the readers see them from the
    // owner's tail or its commit.
    bulk(&cluster, 0, &ids(0..20), 1);
    bulk(&cluster, 1, &ids(0..5), 2);
    let owner = cluster
        .owner_slot("logs", 0)
        .expect("the shard has an owner");
    for slot in 0..3 {
        converges(&cluster, slot, None, 20);
        converges(&cluster, slot, Some(2), 5);
    }
    cluster.kill(owner);
    let survivors: Vec<usize> = (0..3).filter(|s| *s != owner).collect();
    wait_for("the others never saw the owner go", 30, || {
        survivors.iter().all(|s| cluster.nodes_seen(*s) == 2)
    });
    (cluster, owner)
}

#[test]
fn a_search_after_the_owner_died_never_counts_fewer_than_were_acknowledged_without_a_failure() {
    let _alone = alone();
    let (cluster, owner) = killed_owner();
    for slot in (0..3).filter(|s| *s != owner) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(3) {
            if let Some((total, failed)) = count(&cluster, slot, None) {
                assert!(
                    failed > 0 || total == 20,
                    "node {slot} answered {total} with no failed shard"
                );
                assert!(total <= 20, "node {slot} counted {total}");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[test]
fn after_the_takeover_nothing_acknowledged_is_lost_doubled_or_stale() {
    let _alone = alone();
    let (cluster, owner) = killed_owner();
    let survivors: Vec<usize> = (0..3).filter(|s| *s != owner).collect();
    // A write through a survivor makes the new owner take the shard: five
    // new documents, and five updated ones.
    bulk(&cluster, survivors[0], &ids(20..25), 1);
    bulk(&cluster, survivors[0], &ids(5..10), 3);
    for slot in &survivors {
        converges(&cluster, *slot, None, 25);
        converges(&cluster, *slot, Some(2), 5);
        converges(&cluster, *slot, Some(3), 5);
        converges(&cluster, *slot, Some(1), 15);
    }
}

#[test]
fn the_overlay_is_rebuilt_against_the_successor_so_a_refreshed_write_is_never_missed() {
    let _alone = alone();
    let (cluster, owner) = killed_owner();
    let survivors: Vec<usize> = (0..3).filter(|s| *s != owner).collect();
    bulk(&cluster, survivors[0], &ids(20..21), 1);
    for round in 0_u64..10 {
        let id = format!("fresh{round}");
        bulk(&cluster, survivors[0], std::slice::from_ref(&id), 7);
        for slot in &survivors {
            // refresh=true waits for the readers, so the first search counts it.
            let (total, failed) = answered(&cluster, *slot, Some(7));
            assert_eq!(failed, 0, "round {round}, node {slot}");
            assert_eq!(total, round + 1, "round {round}, node {slot}");
        }
    }
}

/// The node that owns a shard searches it too: a refreshed write is counted
/// by its own search, and by every other node's, from the first search.
#[test]
fn the_owner_counts_its_own_refreshed_writes_from_its_first_search() {
    let _alone = alone();
    let cluster = started();
    bulk(&cluster, 0, &ids(0..3), 1);
    for round in 0_u64..6 {
        let id = format!("fresh{round}");
        bulk(&cluster, 0, std::slice::from_ref(&id), 7);
        for slot in 0..3 {
            assert_eq!(
                answered(&cluster, slot, Some(7)),
                (round + 1, 0),
                "round {round}, node {slot}"
            );
        }
    }
}

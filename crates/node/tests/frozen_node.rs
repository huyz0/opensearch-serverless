//! docs/specs/frozen-node-suite: a shard owner whose object-store traffic is
//! held past its lease, so a successor takes the shard and the owner's late
//! requests arrive afterwards, loses and doubles nothing it acknowledged.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

mod suite_rig;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use suite_rig::{Cluster, Freezer, wait_for};

const TTL_MS: u64 = 3_000;

/// One cluster at a time: leases last three seconds, and clusters of three
/// nodes on one machine starve each other's renewals.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn alone() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A TCP server on a free port that answers every byte with itself.
fn echo_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let addr = listener.local_addr().expect("an address").to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buf = [0u8; 4096];
                while let Ok(n) = stream.read(&mut buf) {
                    if n == 0 || stream.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

fn read_exactly(stream: &mut TcpStream, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    stream.read_exact(&mut out).expect("the bytes arrive");
    out
}

#[test]
fn a_frozen_freezer_holds_bytes_and_delivers_them_whole_and_in_order_on_thaw() {
    let freezer = Freezer::start(&echo_server());
    let addr = freezer.endpoint().replace("http://", "");

    // A connection open before the freeze.
    let mut before = TcpStream::connect(&addr).expect("connects");
    before.write_all(b"ping").expect("writes");
    assert_eq!(read_exactly(&mut before, 4), b"ping");

    freezer.freeze();
    // 100 KB, several chunks, a pattern that shows a reordering or a loss.
    let payload: Vec<u8> = (0..100_000_u32).map(|i| (i % 251) as u8).collect();
    before.write_all(&payload).expect("writes");
    // A connection opened while frozen.
    let mut during = TcpStream::connect(&addr).expect("connects");
    during.write_all(b"late").expect("writes");

    before
        .set_read_timeout(Some(Duration::from_millis(400)))
        .expect("a timeout");
    let mut one = [0u8; 1];
    let held = before.read(&mut one);
    assert!(
        matches!(&held, Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)),
        "a frozen freezer answered: {held:?}"
    );

    freezer.thaw();
    before
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("a timeout");
    assert_eq!(read_exactly(&mut before, payload.len()), payload);
    during
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("a timeout");
    assert_eq!(read_exactly(&mut during, 4), b"late");
}

fn started() -> Cluster {
    let mut cluster = Cluster::new(TTL_MS, 4, false).expect("emulated");
    cluster.set("OSS_TAIL_OVERLAY", "true");
    cluster.set("OSS_REFRESH_VISIBILITY", "everywhere");
    for id in ["n1", "n2", "n3"] {
        cluster.add_behind(id);
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

fn body(ids: &[String], n: u32) -> String {
    ids.iter()
        .map(|id| {
            format!("{{\"index\":{{\"_index\":\"logs\",\"_id\":\"{id}\"}}}}\n{{\"n\":{n}}}\n")
        })
        .collect()
}

/// Indexes `ids` with `refresh=true` until every item is created or 60 s
/// pass; indexing by id again is the same write.
fn bulk(cluster: &Cluster, slot: usize, ids: &[String], n: u32) {
    let body = body(ids, n);
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

/// The hit total and failed shards of a search for `value` (all documents
/// when `None`), or `None` while the search is refused.
fn count(cluster: &Cluster, slot: usize, value: Option<u32>) -> Option<(u64, u64)> {
    let query = value.map_or_else(
        || r#"{"match_all":{}}"#.to_owned(),
        |v| format!(r#"{{"term":{{"n":{v}}}}}"#),
    );
    let request = format!(r#"{{"query":{query},"size":0,"track_total_hits":true}}"#);
    let r = cluster.call(slot, "POST", "/logs/_search", Some(&request));
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

/// Polls until `slot` counts `want` with no failed shard, failing if a poll
/// ever counts more (a document seen twice).
fn converges(cluster: &Cluster, slot: usize, value: Option<u32>, want: u64) {
    let started = Instant::now();
    let mut last = None;
    loop {
        if let Some((total, failed)) = count(cluster, slot, value) {
            last = Some((total, failed));
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

#[test]
fn a_write_held_at_a_frozen_owner_is_counted_once_or_not_at_all_after_the_thaw() {
    let _alone = alone();
    let cluster = started();
    bulk(&cluster, 0, &ids(0..20), 1);
    let owner = cluster
        .owner_slot("logs", 0)
        .expect("the shard has an owner");
    for slot in 0..3 {
        converges(&cluster, slot, None, 20);
    }
    let survivors: Vec<usize> = (0..3).filter(|s| *s != owner).collect();

    cluster.freeze(owner);
    let frozen_at = Instant::now();
    let answered = AtomicBool::new(false);
    let held = ids(100..103);

    let reply = std::thread::scope(|scope| {
        // A write sent to the frozen owner: its append is held at the proxy.
        let sender = scope.spawn(|| {
            let r = cluster.call(owner, "POST", "/_bulk?refresh=true", Some(&body(&held, 9)));
            answered.store(true, Ordering::SeqCst);
            r
        });

        wait_for("the others never saw the owner go", 25, || {
            survivors.iter().all(|s| cluster.nodes_seen(*s) == 2)
        });
        // The owner's lease is out and the survivors have dropped it, so a
        // write is not forwarded to a node that cannot answer.
        std::thread::sleep(Duration::from_millis(TTL_MS * 2));
        // A survivor's write takes the shard.
        bulk(&cluster, survivors[0], &ids(20..25), 1);
        for slot in &survivors {
            converges(&cluster, *slot, None, 25);
        }
        assert!(
            !answered.load(Ordering::SeqCst),
            "the frozen owner acknowledged a write before the thaw"
        );
        assert!(
            frozen_at.elapsed() < Duration::from_secs(28),
            "the freeze outlasted the store client's timeout"
        );
        cluster.thaw(owner);
        sender.join().expect("the sender finishes")
    });

    // The owner's append reaches the log after the successor fenced its
    // slot, so it is refused as fenced and nothing of it is counted.
    assert_eq!(reply.status, 200, "{}", reply.text());
    let text = reply.text();
    assert!(text.contains("\"errors\":true"), "acknowledged: {text}");
    assert!(text.contains("was fenced at slot"), "not fenced: {text}");
    // The thawed node serves a write for the shard by passing it to the owner.
    bulk(&cluster, owner, &ids(25..26), 1);
    for slot in 0..3 {
        converges(&cluster, slot, None, 26);
    }
    // No node counts a document of the refused write.
    for slot in 0..3 {
        converges(&cluster, slot, Some(9), 0);
    }
}

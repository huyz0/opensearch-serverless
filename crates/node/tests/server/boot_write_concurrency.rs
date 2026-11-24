//! docs/specs/write-concurrency: write requests in flight together share a
//! commit window, over a booted node and a file store.

use crate::support;

use std::time::{Duration, Instant};

use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

const CLIENTS: usize = 16;

fn boot(root: &std::path::Path) -> Node {
    let root = root.display().to_string();
    let lookup = |key: &str| match key {
        "OSS_NODE_ID" => Some("n1".to_owned()),
        "OSS_STORE" => Some("fs".to_owned()),
        "OSS_FS_ROOT" => Some(root.clone()),
        "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
        _ => None,
    };
    start(&NodeConfig::from_lookup(lookup).expect("valid")).expect("starts")
}

/// Runs `CLIENTS` requests at once and answers how long they took together.
fn together(node: &Node, work: impl Fn(usize) + Sync) -> Duration {
    let _ = node;
    let started = Instant::now();
    std::thread::scope(|s| {
        for c in 0..CLIENTS {
            let work = &work;
            s.spawn(move || work(c));
        }
    });
    started.elapsed()
}

// The window is 250 ms; one request after another needs `CLIENTS` of them.
const SERIAL: Duration = Duration::from_millis(250 * CLIENTS as u64);
const BOUND: Duration = Duration::from_millis(2_500);

// AC-3
#[test]
fn bulks_to_different_indexes_in_flight_together_share_a_window() {
    let dir = TempDir::new("write-concurrency-bulk");
    let node = boot(dir.path());
    let addr = node.addr();
    for c in 0..CLIENTS {
        assert_eq!(
            support::request(addr, "PUT", &format!("/b{c}"), None).status,
            200
        );
        // Opens the shard, outside the clock.
        let warm = format!("{{\"index\":{{\"_index\":\"b{c}\"}}}}\n{{\"n\":0}}\n");
        assert_eq!(
            support::request(addr, "POST", "/_bulk", Some(warm.as_bytes())).status,
            200
        );
    }
    let took = together(&node, |c| {
        let body = format!("{{\"index\":{{\"_index\":\"b{c}\"}}}}\n{{\"n\":1}}\n");
        let reply = support::request(addr, "POST", "/_bulk", Some(body.as_bytes()));
        assert_eq!(reply.status, 200, "{}", reply.text());
    });
    assert!(
        took < BOUND,
        "{took:?} for {CLIENTS} bulks; serialised they need {SERIAL:?}"
    );
}

// AC-3
#[test]
fn single_document_writes_in_flight_together_share_a_window() {
    let dir = TempDir::new("write-concurrency-doc");
    let node = boot(dir.path());
    let addr = node.addr();
    for c in 0..CLIENTS {
        assert_eq!(
            support::request(addr, "PUT", &format!("/d{c}"), None).status,
            200
        );
        let warm = support::request(addr, "POST", &format!("/d{c}/_doc"), Some(br#"{"n":0}"#));
        assert_eq!(warm.status, 201, "{}", warm.text());
    }
    let took = together(&node, |c| {
        let reply = support::request(addr, "POST", &format!("/d{c}/_doc"), Some(br#"{"n":1}"#));
        assert_eq!(reply.status, 201, "{}", reply.text());
    });
    assert!(
        took < BOUND,
        "{took:?} for {CLIENTS} writes; serialised they need {SERIAL:?}"
    );
}

// AC-4
#[test]
fn concurrent_bulks_to_one_index_give_every_document_its_own_id() {
    let dir = TempDir::new("write-concurrency-ids");
    let node = boot(dir.path());
    let addr = node.addr();
    assert_eq!(support::request(addr, "PUT", "/same", None).status, 200);
    let per = 50;
    together(&node, |_| {
        let mut body = String::new();
        for i in 0..per {
            body.push_str(&format!(
                "{{\"index\":{{\"_index\":\"same\"}}}}\n{{\"n\":{i}}}\n"
            ));
        }
        let reply = support::request(addr, "POST", "/_bulk", Some(body.as_bytes()));
        assert_eq!(reply.status, 200, "{}", reply.text());
    });
    assert_eq!(
        support::request(addr, "POST", "/same/_refresh", None).status,
        200
    );
    let count = support::request(addr, "GET", "/same/_count", None).text();
    assert!(
        count.contains(&format!("\"count\":{}", CLIENTS * per)),
        "{count}"
    );
}

const MANY: usize = 32;

// AC-5
#[test]
fn more_bulks_than_four_in_flight_together_share_a_window() {
    let dir = TempDir::new("write-concurrency-many");
    let node = boot(dir.path());
    let addr = node.addr();
    for c in 0..MANY {
        assert_eq!(
            support::request(addr, "PUT", &format!("/m{c}"), None).status,
            200
        );
        let warm = format!("{{\"index\":{{\"_index\":\"m{c}\"}}}}\n{{\"n\":0}}\n");
        assert_eq!(
            support::request(addr, "POST", "/_bulk", Some(warm.as_bytes())).status,
            200
        );
    }
    let started = Instant::now();
    std::thread::scope(|s| {
        for c in 0..MANY {
            s.spawn(move || {
                let body = format!("{{\"index\":{{\"_index\":\"m{c}\"}}}}\n{{\"n\":1}}\n");
                let reply = support::request(addr, "POST", "/_bulk", Some(body.as_bytes()));
                assert_eq!(reply.status, 200, "{}", reply.text());
            });
        }
    });
    let took = started.elapsed();
    // Held four at a time, the 32 need eight windows (2 s).
    assert!(
        took < Duration::from_millis(1_200),
        "{took:?} for {MANY} bulks; four at a time they need 2 s"
    );
}

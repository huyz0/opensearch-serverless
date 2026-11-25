//! docs/specs/write-concurrency: write requests in flight together share a
//! commit window, over a booted node and a file store.

use crate::support;

use std::time::{Duration, Instant};

use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

const CLIENTS: usize = 16;

fn boot(root: &std::path::Path) -> Node {
    boot_with(root, "1")
}

fn boot_with(root: &std::path::Path, depth: &str) -> Node {
    let root = root.display().to_string();
    let lookup = |key: &str| match key {
        "OSS_NODE_ID" => Some("n1".to_owned()),
        "OSS_STORE" => Some("fs".to_owned()),
        "OSS_FS_ROOT" => Some(root.clone()),
        "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
        "OSS_WRITE_PIPELINE_DEPTH" => Some(depth.to_owned()),
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
    // A refresh answers before the search side has the commit (refresh
    // visibility `owner`), so the count is asked for until it is there.
    let t0 = Instant::now();
    let mut count = support::request(addr, "GET", "/same/_count", None).text();
    while !count.contains("\"count\"") && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
        count = support::request(addr, "GET", "/same/_count", None).text();
    }
    assert!(
        count.contains(&format!("\"count\":{}", CLIENTS * per)),
        "{count}"
    );
}

/// 16 clients writing one index at once; answers how long they took.
fn one_index(depth: &str) -> (Duration, Node) {
    let dir = TempDir::new("write-pipeline-one-index");
    let node = boot_with(dir.path(), depth);
    let addr = node.addr();
    assert_eq!(support::request(addr, "PUT", "/one", None).status, 200);
    let warm = "{\"index\":{\"_index\":\"one\",\"_id\":\"warm\"}}\n{\"n\":0}\n";
    assert_eq!(
        support::request(addr, "POST", "/_bulk", Some(warm.as_bytes())).status,
        200
    );
    let took = together(&node, |c| {
        let body = format!("{{\"index\":{{\"_index\":\"one\",\"_id\":\"c{c}\"}}}}\n{{\"n\":1}}\n");
        let reply = support::request(addr, "POST", "/_bulk", Some(body.as_bytes()));
        assert_eq!(reply.status, 200, "{}", reply.text());
        assert!(
            !reply.text().contains("\"errors\":true"),
            "{}",
            reply.text()
        );
    });
    for c in 0..CLIENTS {
        let got = support::request(addr, "GET", &format!("/one/_doc/c{c}"), None);
        assert_eq!(got.status, 200, "c{c}: {}", got.text());
    }
    // Keeps the node alive until every read is made.
    (took, node)
}

// AC-7
#[test]
fn writes_to_one_index_share_a_window_with_a_pipeline_and_not_without_one() {
    let (alone, _n) = one_index("1");
    assert!(
        alone > Duration::from_millis(3_000),
        "at depth 1 the {CLIENTS} bulks take a window each, took {alone:?}"
    );
    let (together, _m) = one_index("16");
    assert!(
        together < Duration::from_millis(1_200),
        "at depth 16 the {CLIENTS} bulks share a window, took {together:?}"
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

const WIDE: usize = 64;

// AC-8
#[test]
fn sixty_four_small_bulks_in_flight_together_share_a_window() {
    let dir = TempDir::new("write-concurrency-wide");
    let node = boot_with(dir.path(), "2");
    let addr = node.addr();
    for c in 0..WIDE {
        assert_eq!(
            support::request(addr, "PUT", &format!("/x{c}"), None).status,
            200
        );
        let warm = format!("{{\"index\":{{\"_index\":\"x{c}\"}}}}\n{{\"n\":0}}\n");
        assert_eq!(
            support::request(addr, "POST", "/_bulk", Some(warm.as_bytes())).status,
            200
        );
    }
    let started = Instant::now();
    std::thread::scope(|s| {
        for c in 0..WIDE {
            s.spawn(move || {
                let body = format!("{{\"index\":{{\"_index\":\"x{c}\"}}}}\n{{\"n\":1}}\n");
                let reply = support::request(addr, "POST", "/_bulk", Some(body.as_bytes()));
                assert_eq!(reply.status, 200, "{}", reply.text());
                assert!(
                    reply.text().contains("\"errors\":false"),
                    "{}",
                    reply.text()
                );
            });
        }
    });
    let took = started.elapsed();
    // With a pipeline each holds a share of its body, not a whole 8 MiB slice; without one, eight fit the 64 MiB budget: eight windows.
    assert!(
        took < Duration::from_millis(1_200),
        "{took:?} for {WIDE} bulks; eight at a time they need 2 s"
    );
}

// AC-15
#[test]
fn a_bulk_with_one_large_item_among_small_ones_is_applied_with_a_pipeline() {
    let dir = TempDir::new("write-concurrency-large-item");
    let node = boot_with(dir.path(), "8");
    let addr = node.addr();
    assert_eq!(support::request(addr, "PUT", "/big", None).status, 200);
    // One item far larger than the share its small body would suggest, and
    // fifty tiny ones around it.
    let mut body = String::new();
    for n in 0..25 {
        body.push_str(&format!(
            "{{\"index\":{{\"_id\":\"s{n}\"}}}}\n{{\"n\":{n}}}\n"
        ));
    }
    body.push_str(&format!(
        "{{\"index\":{{\"_id\":\"large\"}}}}\n{{\"text\":\"{}\"}}\n",
        "x".repeat(300 << 10)
    ));
    for n in 25..50 {
        body.push_str(&format!(
            "{{\"index\":{{\"_id\":\"s{n}\"}}}}\n{{\"n\":{n}}}\n"
        ));
    }
    let reply = support::request(addr, "POST", "/big/_bulk", Some(body.as_bytes()));
    assert_eq!(reply.status, 200, "{}", reply.text());
    let text = reply.text();
    assert!(
        text.contains("\"errors\":false"),
        "{}",
        &text[..text.len().min(400)]
    );
    assert_eq!(text.matches("\"result\":\"created\"").count(), 51);
}

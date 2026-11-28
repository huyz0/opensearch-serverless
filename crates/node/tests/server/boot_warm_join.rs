//! docs/specs/reader-warm-copy: a reader joining a cluster takes its peer's
//! hot pages into its own disk tier before it says it is ready.

// ARITH: the sums and offsets are of small test numbers; an overflow panics
// the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use std::time::{Duration, Instant};

use crate::support;

use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

fn boot(root: &str, id: &str, tier: &std::path::Path, warm: bool) -> Node {
    boot_at(root, id, tier, warm, None)
}

/// As `boot`, with the copy's rate in bytes a second when `rate` is given.
fn boot_at(root: &str, id: &str, tier: &std::path::Path, warm: bool, rate: Option<&str>) -> Node {
    let tier = tier.display().to_string();
    let config = NodeConfig::from_lookup(|key| {
        Some(
            match key {
                "OSS_NODE_ID" => id,
                "OSS_STORE" => "fs",
                "OSS_FS_ROOT" => root,
                "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => "127.0.0.1:0",
                "OSS_LEASE_TTL_MS" => "600",
                "OSS_PAGE_CACHE_DIR" => &tier,
                "OSS_PAGE_CACHE_BYTES" => "67108864",
                "OSS_WARM_ON_JOIN" if warm => "true",
                "OSS_WARM_MAX_WAIT_SECONDS" => "30",
                "OSS_STORE_USAGE" => "true",
                "OSS_WARM_BYTES_PER_SECOND" if rate.is_some() => rate.unwrap_or("1"),
                _ => return None,
            }
            .to_owned(),
        )
    })
    .expect("valid");
    start(&config).expect("starts")
}

fn call(node: &Node, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(node.addr(), method, target, body.map(str::as_bytes))
}

/// Bytes of files under `dir`, as the disk reports them in use.
fn used(dir: &std::path::Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let mut total = 0_u64;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total = total.saturating_add(used(&path));
        } else if let Ok(m) = path.metadata() {
            total = total.saturating_add(m.blocks().saturating_mul(512));
        }
    }
    total
}

fn wait_ready(node: &Node) {
    let started = Instant::now();
    while call(node, "GET", "/_serverless/ready", None).status != 200 {
        assert!(started.elapsed() < Duration::from_secs(40), "never ready");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn search(node: &Node) {
    assert_eq!(call(node, "PUT", "/logs", None).status, 200);
    let mut bulk = String::new();
    for i in 0..200 {
        bulk.push_str(&format!("{{\"index\":{{\"_id\":\"{i}\"}}}}\n"));
        bulk.push_str(&format!(
            "{{\"n\":{i},\"msg\":\"line number {i} of the log\"}}\n"
        ));
    }
    let put = call(node, "POST", "/logs/_bulk?refresh=true", Some(&bulk));
    assert_eq!(put.status, 200, "{}", put.text());
    let found = call(
        node,
        "POST",
        "/logs/_search",
        Some(r#"{"query":{"match_all":{}},"size":3}"#),
    );
    assert_eq!(found.status, 200, "{}", found.text());
}

// docs/specs/reader-warm-copy AC-5, AC-7
#[test]
fn a_joining_reader_holds_its_peers_pages_when_it_says_it_is_ready() {
    let store = TempDir::new("warm-join-store");
    let root = store.path().display().to_string();
    let tier_a = TempDir::new("warm-join-a");
    let tier_b = TempDir::new("warm-join-b");

    let a = boot(&root, "n1", tier_a.path(), true);
    wait_ready(&a);
    search(&a);
    assert!(used(tier_a.path()) > 0, "the first node cached pages");

    let b = boot(&root, "n2", tier_b.path(), true);
    let started = Instant::now();
    let ready = loop {
        let reply = call(&b, "GET", "/_serverless/ready", None);
        if reply.status == 200 {
            break reply;
        }
        assert!(
            reply.text().contains(r#""state":"warming""#),
            "{}",
            reply.text()
        );
        assert!(started.elapsed() < Duration::from_secs(40), "never ready");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(ready.text().contains(r#""ready":true"#), "{}", ready.text());
    assert!(
        used(tier_b.path()) > 0,
        "the joiner took pages into its tier before it was ready"
    );
}

// docs/specs/reader-warm-copy AC-7
#[test]
fn a_node_not_asked_to_warm_is_ready_at_once_and_holds_nothing() {
    let store = TempDir::new("warm-join-off");
    let root = store.path().display().to_string();
    let tier_a = TempDir::new("warm-join-off-a");
    let tier_b = TempDir::new("warm-join-off-b");

    let a = boot(&root, "n1", tier_a.path(), true);
    search(&a);
    let b = boot(&root, "n2", tier_b.path(), false);
    let ready = call(&b, "GET", "/_serverless/ready", None);
    assert_eq!(ready.status, 200, "{}", ready.text());
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(used(tier_b.path()), 0, "switch off: no pages taken");
}

/// The GETs, ranged GETs and HEADs a node has made for `index`, from its
/// usage report.
fn reads(node: &Node, index: &str) -> f64 {
    let body = call(node, "GET", "/_serverless/usage", None).text();
    // An index the node has not touched is not listed: no reads.
    let Some(at) = body.find(&format!("\"{index}\":{{\"cost_units\"")) else {
        return 0.0;
    };
    let rest = &body[at..];
    ["\"get\":", "\"get_range\":", "\"head\":"]
        .iter()
        .map(|key| {
            let from = rest.find(key).expect("a kind") + key.len();
            let to = rest[from..].find([',', '}']).expect("an end") + from;
            rest[from..to].parse::<f64>().expect("a number")
        })
        .sum()
}

fn first_search(node: &Node) -> Duration {
    let started = Instant::now();
    let found = call(
        node,
        "POST",
        "/logs/_search",
        Some(r#"{"query":{"match":{"msg":"line"}},"size":3}"#),
    );
    assert_eq!(found.status, 200, "{}", found.text());
    started.elapsed()
}

// docs/specs/reader-warm-copy AC-8: what the first search of a joining reader
// costs with the warm-up and without it, over the same shard.
#[test]
fn the_first_search_of_a_warmed_reader_makes_fewer_store_reads_than_a_cold_one() {
    let store = TempDir::new("warm-join-measure");
    let root = store.path().display().to_string();
    let tiers = ["a", "b", "c"].map(|n| TempDir::new(&format!("warm-join-measure-{n}")));

    let a = boot(&root, "n1", tiers[0].path(), true);
    wait_ready(&a);
    search(&a);

    let b = boot(&root, "n2", tiers[1].path(), true);
    wait_ready(&b);
    let warm_up = reads(&b, "logs");
    let warm_search_time = first_search(&b);
    let warm_search = reads(&b, "logs") - warm_up;

    let c = boot(&root, "n3", tiers[2].path(), false);
    wait_ready(&c);
    let before = reads(&c, "logs");
    let cold_search_time = first_search(&c);
    let cold_search = reads(&c, "logs") - before;

    eprintln!(
        "AC-8 store reads for logs: warm-up {warm_up}, first search after it {warm_search} \
         ({warm_search_time:?}); cold first search {cold_search} ({cold_search_time:?})"
    );
    assert!(
        warm_search < cold_search,
        "warm {warm_search} against cold {cold_search}"
    );
    assert!(
        warm_up + warm_search <= cold_search,
        "the warm-up and the search together read no more than the cold search"
    );
}

/// The roles `id`'s lease names in the store at `root`; empty with no lease.
fn lease_roles(root: &std::path::Path, id: &str) -> Vec<String> {
    use oss_blob::BlobStore;
    let store = oss_blob::FsStore::new(root).expect("store");
    let key = oss_format::lease_key("", id).expect("key");
    let mut buf = vec![0u8; 1 << 16];
    let Ok(Some(read)) = store.read_register(&key, &mut buf) else {
        return Vec::new();
    };
    oss_format::NodeLease::parse(&buf[..read.len])
        .map(|l| l.roles().to_vec())
        .unwrap_or_default()
}

// docs/specs/reader-warm-copy AC-7
#[test]
fn a_warming_node_has_no_search_role_in_its_lease_until_it_is_ready() {
    let store = TempDir::new("warm-lease-store");
    let root = store.path().display().to_string();
    let tier_a = TempDir::new("warm-lease-a");
    let tier_b = TempDir::new("warm-lease-b");

    let a = boot(&root, "n1", tier_a.path(), true);
    wait_ready(&a);
    // Enough pages for several batches, so the copy, paced below, outlasts
    // several lease refreshes.
    let body = r#"{"settings":{"index":{"number_of_shards":8}}}"#;
    assert_eq!(call(&a, "PUT", "/logs", Some(body)).status, 200);
    for chunk in (0..16_000).collect::<Vec<_>>().chunks(2000) {
        let mut bulk = String::new();
        for i in chunk {
            bulk.push_str(&format!("{{\"index\":{{\"_id\":\"{i}\"}}}}\n"));
            bulk.push_str(&format!(
                "{{\"n\":{i},\"msg\":\"line {i} of the log with some words w{} x{}\"}}\n",
                i % 97,
                i % 31
            ));
        }
        let put = call(&a, "POST", "/logs/_bulk?refresh=true", Some(&bulk));
        assert_eq!(put.status, 200, "{}", put.text());
    }
    for i in 0..300 {
        let q = format!(
            r#"{{"query":{{"match":{{"msg":"w{}"}}}},"size":10}}"#,
            i % 97
        );
        assert_eq!(call(&a, "POST", "/logs/_search", Some(&q)).status, 200);
    }
    let b = boot_at(&root, "n2", tier_b.path(), true, Some("65536"));
    let mut warming_polls = 0;
    let started = Instant::now();
    loop {
        // The lease first: a search role seen here was written after the node
        // finished warming, so the probe asked next cannot say warming.
        let roles = lease_roles(store.path(), "n2");
        let ready = call(&b, "GET", "/_serverless/ready", None).status == 200;
        if ready {
            break;
        }
        assert!(
            !roles.iter().any(|r| r == "search"),
            "a warming node offered search after {warming_polls} polls: {roles:?}"
        );
        warming_polls += 1;
        assert!(started.elapsed() < Duration::from_secs(40), "never ready");
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(
        warming_polls >= 5,
        "the node was warming for only {warming_polls} polls, so the lease was barely looked at"
    );
    let until = Instant::now();
    while !lease_roles(store.path(), "n2")
        .iter()
        .any(|r| r == "search")
    {
        assert!(
            until.elapsed() < Duration::from_secs(5),
            "ready, but the lease never named search"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
}

// docs/specs/reader-warm-copy AC-3, AC-5: the peer serves the copy from its
// cache and makes no store request for it; the joiner's search after the
// warm-up finds its pages with no further read.
#[test]
fn serving_the_copy_costs_the_peer_no_store_request_and_the_joiners_search_reads_nothing() {
    let store = TempDir::new("warm-join-requests");
    let root = store.path().display().to_string();
    let tiers = ["a", "b"].map(|n| TempDir::new(&format!("warm-join-requests-{n}")));

    let a = boot(&root, "n1", tiers[0].path(), true);
    wait_ready(&a);
    search(&a);
    let peer_before = reads(&a, "logs");
    assert!(peer_before > 0.0, "the peer's own reads are metered");

    let b = boot(&root, "n2", tiers[1].path(), true);
    wait_ready(&b);
    assert!(used(tiers[1].path()) > 0, "the joiner took pages");
    let peer_after = reads(&a, "logs");
    assert_eq!(
        peer_after,
        peer_before,
        "the peer made {} store requests while it served the copy",
        peer_after - peer_before
    );

    let warm_up = reads(&b, "logs");
    first_search(&b);
    assert_eq!(
        reads(&b, "logs"),
        warm_up,
        "the search after the warm-up read the store"
    );
    // A second search is as free as the first.
    first_search(&b);
    assert_eq!(reads(&b, "logs"), warm_up);
}

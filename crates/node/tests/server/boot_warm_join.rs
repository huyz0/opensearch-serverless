//! docs/specs/reader-warm-copy: a reader joining a cluster takes its peer's
//! hot pages into its own disk tier before it says it is ready.

use std::time::{Duration, Instant};

use crate::support;

use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

fn boot(root: &str, id: &str, tier: &std::path::Path, warm: bool) -> Node {
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

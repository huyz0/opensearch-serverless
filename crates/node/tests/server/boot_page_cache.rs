//! docs/specs/node-page-cache-disk: the page cache's disk tier on a booted
//! node.

use crate::support;

use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

fn config(root: &str, extra: &[(&str, &str)]) -> Result<NodeConfig, String> {
    NodeConfig::from_lookup(|key| {
        if let Some((_, v)) = extra.iter().find(|(k, _)| *k == key) {
            return Some((*v).to_owned());
        }
        match key {
            "OSS_NODE_ID" => Some("n1".to_owned()),
            "OSS_STORE" => Some("fs".to_owned()),
            "OSS_FS_ROOT" => Some(root.to_owned()),
            "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
            _ => None,
        }
    })
}

fn boot(root: &std::path::Path, extra: &[(&str, &str)]) -> Node {
    let root = root.display().to_string();
    start(&config(&root, extra).expect("valid")).expect("starts")
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

/// Indexes documents, refreshes, and answers a search over them.
fn search(node: &Node) -> String {
    assert_eq!(call(node, "PUT", "/logs", None).status, 200);
    let mut bulk = String::new();
    for i in 0..50 {
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
    found.text()
}

// AC-1
#[test]
fn the_disk_tier_is_off_unless_asked_for_and_asked_for_with_a_size() {
    let off = config("/tmp/unused", &[]).expect("valid");
    assert_eq!(off.page_cache, None);
    for (dir, bytes) in [
        ("/tmp/pc", "0"),
        ("/tmp/pc", "1048575"),
        ("/tmp/pc", "soon"),
    ] {
        assert!(
            config(
                "/tmp/unused",
                &[("OSS_PAGE_CACHE_DIR", dir), ("OSS_PAGE_CACHE_BYTES", bytes)]
            )
            .is_err(),
            "{dir} {bytes}"
        );
    }
    assert!(config("/tmp/unused", &[("OSS_PAGE_CACHE_DIR", "/tmp/pc")]).is_err());
    let on = config(
        "/tmp/unused",
        &[
            ("OSS_PAGE_CACHE_DIR", "/tmp/pc"),
            ("OSS_PAGE_CACHE_BYTES", "1048576"),
        ],
    )
    .expect("valid");
    assert_eq!(on.page_cache.map(|d| d.bytes), Some(1 << 20));
}

// AC-2
#[test]
fn a_node_with_a_disk_tier_keeps_the_pages_it_read_and_answers_the_same() {
    let plain = TempDir::new("boot-page-cache-off");
    let tier = TempDir::new("boot-page-cache-on");
    let tier_dir = tier.path().join("pages");
    let tier_dir_str = tier_dir.display().to_string();
    let answer_off = search(&boot(plain.path(), &[]));
    let root = TempDir::new("boot-page-cache-root");
    let node = boot(
        root.path(),
        &[
            ("OSS_PAGE_CACHE_DIR", &tier_dir_str),
            ("OSS_PAGE_CACHE_BYTES", "67108864"),
        ],
    );
    let answer_on = search(&node);
    let total = |s: &str| {
        let at = s.find("\"total\":{").expect("total");
        let rest = &s[at..];
        rest[..rest.find('}').expect("end") + 1].to_owned()
    };
    assert_eq!(total(&answer_on), total(&answer_off));
    assert!(total(&answer_on).contains("\"value\":50"), "{answer_on}");
    assert!(used(&tier_dir) > 0, "the tier holds what the search read");
}

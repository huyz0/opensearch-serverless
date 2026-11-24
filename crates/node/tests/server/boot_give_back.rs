//! docs/specs/writer-give-back: a node started from configuration gives back
//! the head of a shard asleep long enough.

use crate::support;

use std::time::{Duration, Instant};

use oss_blob::{BlobStore, FsStore};
use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

/// The node the shard head of `logs` shard 0 names as its owner.
fn head_owner(root: &std::path::Path) -> Option<String> {
    let store = FsStore::new(root).expect("store");
    let key = oss_cluster::heads::shard_head_key("", "logs", 0);
    let mut buf = vec![0u8; 1 << 16];
    let read = store.read_register(&key, &mut buf).expect("reads")?;
    oss_format::ShardHead::parse(buf.get(..read.len)?)
        .expect("parses")
        .owner_node_id
}

fn call(node: &Node, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(node.addr(), method, target, body.map(str::as_bytes))
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(20), "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// docs/specs/writer-give-back AC-8
#[test]
fn a_booted_node_gives_back_the_head_of_a_long_asleep_shard_only_when_the_setting_is_on() {
    for give_back in [None, Some("600")] {
        let dir = TempDir::new("boot-give-back");
        let root = dir.path().display().to_string();
        let node = start(
            &NodeConfig::from_lookup(|key| {
                Some(
                    match key {
                        "OSS_NODE_ID" => "n1",
                        "OSS_STORE" => "fs",
                        "OSS_FS_ROOT" => root.as_str(),
                        "OSS_HTTP_ADDR" => "127.0.0.1:0",
                        "OSS_TRANSPORT_ADDR" => "127.0.0.1:0",
                        "OSS_LEASE_TTL_MS" => "3000",
                        "OSS_HIBERNATE_AFTER_MS" => "200",
                        "OSS_GIVE_BACK_AFTER_MS" => return give_back.map(str::to_owned),
                        _ => return None,
                    }
                    .to_owned(),
                )
            })
            .expect("valid"),
        )
        .expect("starts");
        assert_eq!(call(&node, "PUT", "/logs", None).status, 200);
        let wrote = call(&node, "PUT", "/logs/_doc/1", Some(r#"{"n":1}"#));
        assert_eq!(wrote.status, 201, "{}", wrote.text());
        assert_eq!(head_owner(dir.path()).as_deref(), Some("n1"));
        if give_back.is_some() {
            wait_for("the head was never given back", || {
                head_owner(dir.path()).is_none()
            });
        } else {
            // Long enough to have slept and to have been given back.
            std::thread::sleep(Duration::from_secs(4));
            assert_eq!(head_owner(dir.path()).as_deref(), Some("n1"), "off: kept");
        }
        drop(node);
    }
}

//! docs/specs/node-bucket-cost: a node started with the buckets switch on
//! creates, writes, refreshes and searches an index that lives in a bucket.

use crate::support;

use oss_blob::{BlobStore, FsStore};
use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

fn boot(root: &str, buckets: bool) -> Node {
    start(
        &NodeConfig::from_lookup(|key| {
            Some(
                match key {
                    "OSS_NODE_ID" => "n1",
                    "OSS_STORE" => "fs",
                    "OSS_FS_ROOT" => root,
                    "OSS_HTTP_ADDR" => "127.0.0.1:0",
                    "OSS_TRANSPORT_ADDR" => "127.0.0.1:0",
                    "OSS_LEASE_TTL_MS" => "3000",
                    "OSS_BUCKETS_ENABLED" if buckets => "true",
                    _ => return None,
                }
                .to_owned(),
            )
        })
        .expect("valid"),
    )
    .expect("starts")
}

fn call(node: &Node, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(node.addr(), method, target, body.map(str::as_bytes))
}

fn exists(root: &std::path::Path, key: &str) -> bool {
    let store = FsStore::new(root).expect("store");
    store
        .read_register(key, &mut vec![0u8; 1 << 20])
        .expect("reads")
        .is_some()
}

/// Boots a node, creates `logs`, writes a document, refreshes and searches,
/// and returns the store's directory.
fn run(buckets: bool) -> TempDir {
    let dir = TempDir::new("boot-buckets");
    let root = dir.path().display().to_string();
    let node = boot(&root, buckets);
    assert_eq!(call(&node, "PUT", "/logs", None).status, 200);
    let wrote = call(&node, "PUT", "/logs/_doc/1", Some(r#"{"n":1}"#));
    assert_eq!(wrote.status, 201, "{}", wrote.text());
    let refreshed = call(&node, "POST", "/logs/_refresh", None);
    assert_eq!(refreshed.status, 200, "{}", refreshed.text());
    let found = call(
        &node,
        "POST",
        "/logs/_search",
        Some(r#"{"query":{"match_all":{}}}"#),
    );
    assert_eq!(found.status, 200, "{}", found.text());
    let hits = found.text();
    assert!(hits.contains(r#""total":{"value":1"#), "{hits}");
    drop(node);
    dir
}

/// The uuid and bucket of `logs`, from its descriptor.
fn descriptor(dir: &TempDir) -> (String, Option<String>) {
    let store = FsStore::new(dir.path()).expect("store");
    let key = oss_format::descriptor_key("", "logs").expect("key");
    let mut buf = vec![0u8; 1 << 16];
    let read = store
        .read_register(&key, &mut buf)
        .expect("reads")
        .expect("exists");
    let oss_format::DescriptorRecord::Index(index) =
        oss_format::DescriptorRecord::parse(buf.get(..read.len).expect("bytes"))
            .expect("descriptor")
    else {
        panic!("an index");
    };
    (index.uuid.to_string(), index.bucket.map(|b| b.to_string()))
}

#[test]
fn an_index_created_with_the_switch_on_is_written_and_searched_through_its_bucket() {
    let dir = run(true);
    let (uuid, bucket) = descriptor(&dir);
    let bucket = bucket.expect("bucketed");
    let manifest = oss_format::bucket_manifest_key("", &bucket).expect("key");
    assert!(exists(dir.path(), &manifest), "the bucket manifest exists");
    let shard = oss_format::ShardRef {
        index: "logs",
        uuid: &uuid,
        shard: 0,
    };
    assert!(
        !exists(dir.path(), &shard.manifest_key("").expect("key")),
        "the shard has no register of its own"
    );
}

#[test]
fn the_same_requests_with_the_switch_off_leave_a_shard_register() {
    let dir = run(false);
    let (uuid, bucket) = descriptor(&dir);
    assert!(bucket.is_none());
    let shard = oss_format::ShardRef {
        index: "logs",
        uuid: &uuid,
        shard: 0,
    };
    assert!(exists(dir.path(), &shard.manifest_key("").expect("key")));
}

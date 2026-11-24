//! docs/specs/node-roles: a node started with some of the roles.

use crate::support;

use std::time::{Duration, Instant};

use oss_blob::{BlobStore, FsStore};
use oss_cluster::placement::{ROLE_INGEST, ROLE_SEARCH, ShardKey, readers_for};
use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

fn config(root: &str, id: &str, roles: Option<&str>) -> Result<NodeConfig, String> {
    NodeConfig::from_lookup(|key| match key {
        "OSS_NODE_ID" => Some(id.to_owned()),
        "OSS_STORE" => Some("fs".to_owned()),
        "OSS_FS_ROOT" => Some(root.to_owned()),
        "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
        "OSS_LEASE_TTL_MS" => Some("30000".to_owned()),
        "OSS_ROLES" => roles.map(str::to_owned),
        _ => None,
    })
}

fn boot(root: &std::path::Path, id: &str, roles: &str) -> Node {
    let root = root.display().to_string();
    start(&config(&root, id, Some(roles)).expect("valid")).expect("starts")
}

fn call(node: &Node, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(node.addr(), method, target, body.map(str::as_bytes))
}

fn lease_of(root: &std::path::Path, id: &str) -> oss_format::NodeLease {
    let store = FsStore::new(root).expect("store");
    let key = oss_format::lease_key("", id).expect("key");
    let mut buf = vec![0u8; 1 << 16];
    let read = store
        .read_register(&key, &mut buf)
        .expect("reads")
        .expect("present");
    oss_format::NodeLease::parse(&buf[..read.len]).expect("parses")
}

fn head_owner(root: &std::path::Path, index: &str) -> Option<String> {
    let store = FsStore::new(root).expect("store");
    let key = oss_cluster::heads::shard_head_key("", index, 0);
    let mut buf = vec![0u8; 1 << 16];
    let read = store.read_register(&key, &mut buf).expect("reads")?;
    oss_format::ShardHead::parse(buf.get(..read.len)?)
        .expect("parses")
        .owner_node_id
}

fn nodes_seen(node: &Node) -> u64 {
    let text = call(node, "GET", "/_cluster/health", None).text();
    let (_, rest) = text.split_once("\"number_of_nodes\":").expect("nodes");
    rest.chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .expect("a number")
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(20), "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// AC-1
#[test]
fn the_variable_names_the_roles_and_refuses_what_it_does_not_know() {
    let roles = |value: Option<&str>| config("/x", "n1", value).map(|c| c.roles);
    for unset in [None, Some(""), Some("  ")] {
        assert_eq!(roles(unset).expect("valid"), ["ingest", "search"]);
    }
    for (value, want) in [
        ("search", vec!["search"]),
        ("ingest", vec!["ingest"]),
        ("search,ingest", vec!["ingest", "search"]),
        (" ingest , search , ingest ", vec!["ingest", "search"]),
        ("none", vec![]),
    ] {
        assert_eq!(roles(Some(value)).expect(value), want, "{value}");
    }
    for bad in ["bogus", "ingest,bogus", "none,search", "ingest,none"] {
        let err = roles(Some(bad)).expect_err(bad);
        assert!(err.contains("OSS_ROLES"), "{bad}: {err}");
    }
}

// AC-2
#[test]
fn a_booted_nodes_lease_names_its_roles() {
    let dir = TempDir::new("roles-lease");
    let _a = boot(dir.path(), "a", "ingest");
    let _b = boot(dir.path(), "b", "search");
    let _c = boot(dir.path(), "c", "none");
    let _d = boot(dir.path(), "d", "ingest,search");
    let names = |id: &str| lease_of(dir.path(), id).roles().to_vec();
    assert_eq!(names("a"), ["ingest"]);
    assert_eq!(names("b"), ["search"]);
    assert!(names("c").is_empty());
    assert_eq!(names("d"), ["ingest", "search"]);
}

// AC-3, AC-5 and AC-6
#[test]
fn a_search_node_hands_its_writes_to_the_ingest_node_and_serves_what_they_wrote() {
    let dir = TempDir::new("roles-pair");
    let ingest = boot(dir.path(), "ingest-1", "ingest");
    let search = boot(dir.path(), "search-1", "search");
    wait_for("the ingest node never saw the search node", || {
        nodes_seen(&ingest) == 2
    });
    wait_for("the search node never saw the ingest node", || {
        nodes_seen(&search) == 2
    });
    assert_eq!(call(&search, "PUT", "/logs", None).status, 200);
    for id in 0..3 {
        let wrote = call(
            &search,
            "PUT",
            &format!("/logs/_doc/{id}?refresh=true"),
            Some(r#"{"n":1}"#),
        );
        assert_eq!(wrote.status, 201, "id {id}: {}", wrote.text());
    }
    assert_eq!(
        head_owner(dir.path(), "logs").as_deref(),
        Some("ingest-1"),
        "the ingest node holds the shard"
    );
    let found = call(&search, "GET", "/logs/_doc/1", None);
    assert_eq!(found.status, 200, "{}", found.text());
    let hits = call(
        &search,
        "POST",
        "/logs/_search",
        Some(r#"{"query":{"match_all":{}}}"#),
    );
    assert_eq!(hits.status, 200, "{}", hits.text());
    assert!(hits.text().contains("\"value\":3"), "{}", hits.text());
    let leases = [
        lease_of(dir.path(), "ingest-1"),
        lease_of(dir.path(), "search-1"),
    ];
    let readers = readers_for(ShardKey::new("logs", 0), leases.iter(), ROLE_SEARCH, 2);
    assert_eq!(readers, ["search-1"]);
    let writers = readers_for(ShardKey::new("logs", 0), leases.iter(), ROLE_INGEST, 2);
    assert_eq!(writers, ["ingest-1"]);
}

// AC-4
#[test]
fn a_search_node_alone_refuses_a_write_and_holds_no_shard() {
    let dir = TempDir::new("roles-alone");
    let search = boot(dir.path(), "search-1", "search");
    assert_eq!(call(&search, "PUT", "/logs", None).status, 200);
    let wrote = call(&search, "PUT", "/logs/_doc/1", Some(r#"{"n":1}"#));
    assert_eq!(wrote.status, 503, "{}", wrote.text());
    assert_eq!(head_owner(dir.path(), "logs"), None);
}

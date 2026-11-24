//! docs/specs/node-retire: a node told to stop publishes what it holds and
//! gives its heads back.

use crate::support;

use std::time::{Duration, Instant};

use oss_blob::{BlobStore, FsStore};
use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

fn boot(root: &std::path::Path, id: &str) -> Node {
    let root = root.display().to_string();
    start(
        &NodeConfig::from_lookup(|key| {
            Some(
                match key {
                    "OSS_NODE_ID" => id,
                    "OSS_STORE" => "fs",
                    "OSS_FS_ROOT" => root.as_str(),
                    "OSS_HTTP_ADDR" => "127.0.0.1:0",
                    "OSS_TRANSPORT_ADDR" => "127.0.0.1:0",
                    // A lease lifetime far longer than the test, so a take that
                    // waited for it would fail the time bound.
                    "OSS_LEASE_TTL_MS" => "60000",
                    _ => return None,
                }
                .to_owned(),
            )
        })
        .expect("valid"),
    )
    .expect("starts")
}

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

// AC-1, AC-2, AC-5
#[test]
fn a_retired_node_leaves_its_shard_to_the_next_writer_at_once() {
    let dir = TempDir::new("boot-retire");
    let first = boot(dir.path(), "n1");
    assert_eq!(call(&first, "PUT", "/logs", None).status, 200);
    let wrote = call(&first, "PUT", "/logs/_doc/1", Some(r#"{"n":1}"#));
    assert_eq!(wrote.status, 201, "{}", wrote.text());
    assert_eq!(head_owner(dir.path()).as_deref(), Some("n1"));
    let addr = first.addr();
    let retired = first.retire(25_000);
    assert!(retired.failed.is_empty(), "{:?}", retired.failed);
    assert_eq!(retired.hibernated, 1);
    assert_eq!(retired.given_back, 1);
    // AC-1: no owner, the document published.
    assert_eq!(head_owner(dir.path()), None);
    // AC-5: the port is closed.
    assert!(std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_err());
    // AC-2: a second node writes at once, and sees both documents.
    let second = boot(dir.path(), "n2");
    let started = Instant::now();
    let wrote = call(&second, "PUT", "/logs/_doc/2", Some(r#"{"n":2}"#));
    assert_eq!(wrote.status, 201, "{}", wrote.text());
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "waited for the lease"
    );
    assert_eq!(call(&second, "POST", "/logs/_refresh", None).status, 200);
    let count = call(&second, "GET", "/logs/_count", None).text();
    assert!(count.contains("\"count\":2"), "{count}");
}

// AC-8
#[test]
fn a_timeout_of_zero_skips_the_shards_and_still_stops_the_node() {
    let dir = TempDir::new("boot-retire-zero");
    let node = boot(dir.path(), "n1");
    assert_eq!(call(&node, "PUT", "/logs", None).status, 200);
    let wrote = call(&node, "PUT", "/logs/_doc/1", Some(r#"{"n":1}"#));
    assert_eq!(wrote.status, 201, "{}", wrote.text());
    let addr = node.addr();
    let retired = node.retire(0);
    assert_eq!((retired.hibernated, retired.given_back), (0, 0));
    assert!(std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_err());
    assert_eq!(
        head_owner(dir.path()).as_deref(),
        Some("n1"),
        "left to the lease"
    );
}

// AC-6
#[test]
fn a_node_holding_nothing_retires_with_nothing_to_do() {
    let dir = TempDir::new("boot-retire-empty");
    let node = boot(dir.path(), "n1");
    let retired = node.retire(25_000);
    assert_eq!((retired.hibernated, retired.given_back), (0, 0));
    assert!(retired.failed.is_empty() && retired.busy == 0);
}

// AC-7
#[cfg(unix)]
#[test]
fn the_binary_retires_on_sigterm_and_exits_zero() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let dir = TempDir::new("boot-retire-signal");
    let mut child = Command::new(env!("CARGO_BIN_EXE_oss-node"))
        .env("OSS_NODE_ID", "n1")
        .env("OSS_STORE", "fs")
        .env("OSS_FS_ROOT", dir.path())
        .env("OSS_HTTP_ADDR", "127.0.0.1:0")
        .env("OSS_TRANSPORT_ADDR", "127.0.0.1:0")
        .env("OSS_LEASE_TTL_MS", "60000")
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawns");
    let mut lines = BufReader::new(child.stdout.take().expect("piped")).lines();
    let line = lines.next().expect("a line").expect("reads");
    let addr: std::net::SocketAddr = line
        .rsplit(' ')
        .next()
        .expect("an address")
        .parse()
        .expect("parses");
    assert_eq!(support::request(addr, "PUT", "/logs", None).status, 200);
    let wrote = support::request(addr, "PUT", "/logs/_doc/1", Some(br#"{"n":1}"#));
    assert_eq!(wrote.status, 201, "{}", wrote.text());
    assert_eq!(head_owner(dir.path()).as_deref(), Some("n1"));
    let sent = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill runs");
    assert!(sent.success());
    let status = child.wait().expect("exits");
    assert!(status.success(), "{status:?}");
    assert_eq!(head_owner(dir.path()), None);
    let retired = lines.next().expect("a retired line").expect("reads");
    assert!(
        retired.contains("1 shards slept, 1 heads given back"),
        "{retired}"
    );
}

// AC-8
#[test]
fn the_retire_timeout_defaults_to_twenty_five_seconds_and_zero_skips_it() {
    for (value, want) in [
        (None, Some(25_000)),
        (Some("0"), Some(0)),
        (Some("soon"), None),
        (Some("3600001"), None),
    ] {
        let config = NodeConfig::from_lookup(|key| match key {
            "OSS_NODE_ID" => Some("n1".to_owned()),
            "OSS_STORE" => Some("fs".to_owned()),
            "OSS_FS_ROOT" => Some("/tmp/unused".to_owned()),
            "OSS_RETIRE_TIMEOUT_MS" => value.map(str::to_owned),
            _ => None,
        });
        assert_eq!(
            config.ok().map(|c| c.retire_timeout_millis),
            want,
            "{value:?}"
        );
    }
}

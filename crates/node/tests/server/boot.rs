//! docs/specs/node-boot: a node started from configuration, over a file
//! store and real sockets.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

use crate::support;

use std::time::{Duration, Instant};

use std::sync::Arc;

use oss_blob::{BlobStore, FsStore};
use oss_cluster::{LeaseClock, LeaseConfig, Membership, SystemClock};
use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

/// The write-log epochs a node's lease names, read from its register.
fn epochs(root: &std::path::Path, id: &str) -> Vec<i64> {
    let store = FsStore::new(root).expect("store");
    let key = oss_format::lease_key("", id).expect("key");
    let mut buf = vec![0u8; 1 << 16];
    let read = store
        .read_register(&key, &mut buf)
        .expect("reads")
        .expect("present");
    let lease = oss_format::NodeLease::parse(&buf[..read.len]).expect("parses");
    lease.node_log.iter().map(|e| e.epoch).collect()
}

/// The ids on the members index, read by a peer that is not a member.
fn enrolled(root: &std::path::Path) -> Vec<String> {
    let clock: Arc<dyn LeaseClock> = Arc::new(SystemClock);
    let observer = Membership::new(
        Arc::new(FsStore::new(root).expect("store")),
        "",
        clock,
        LeaseConfig::default(),
    )
    .expect("membership");
    let mut ids = observer.enrolled_ids().expect("reads");
    ids.sort();
    ids
}

fn config(root: &std::path::Path, id: &str, ttl: &str) -> NodeConfig {
    config_with(root, id, ttl, None)
}

fn config_with(
    root: &std::path::Path,
    id: &str,
    ttl: &str,
    janitor_ms: Option<&str>,
) -> NodeConfig {
    config_tail(root, id, ttl, janitor_ms, None)
}

/// As `config_with`, with proactive takeover set explicitly.
fn config_proactive(
    root: &std::path::Path,
    id: &str,
    ttl: &str,
    janitor_ms: &str,
    on: bool,
) -> NodeConfig {
    let root = root.display().to_string();
    let lookup = |key: &str| {
        Some(
            match key {
                "OSS_NODE_ID" => id,
                "OSS_STORE" => "fs",
                "OSS_FS_ROOT" => root.as_str(),
                "OSS_HTTP_ADDR" => "127.0.0.1:0",
                "OSS_TRANSPORT_ADDR" => "127.0.0.1:0",
                "OSS_LEASE_TTL_MS" => ttl,
                "OSS_JANITOR_EVERY_MS" => janitor_ms,
                "OSS_PROACTIVE_TAKEOVER" => {
                    if on {
                        "true"
                    } else {
                        "false"
                    }
                }
                _ => return None,
            }
            .to_owned(),
        )
    };
    NodeConfig::from_lookup(lookup).expect("valid")
}

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

/// As `config_with`, with the tail overlay setting and the refresh
/// visibility named.
fn config_tail(
    root: &std::path::Path,
    id: &str,
    ttl: &str,
    janitor_ms: Option<&str>,
    tail: Option<(&str, &str)>,
) -> NodeConfig {
    let root = root.display().to_string();
    let lookup = |key: &str| {
        Some(
            match key {
                "OSS_NODE_ID" => id,
                "OSS_STORE" => "fs",
                "OSS_FS_ROOT" => root.as_str(),
                "OSS_HTTP_ADDR" => "127.0.0.1:0",
                "OSS_TRANSPORT_ADDR" => "127.0.0.1:0",
                // A third of it is the renewal tick.
                "OSS_LEASE_TTL_MS" => ttl,
                "OSS_JANITOR_EVERY_MS" => return janitor_ms.map(str::to_owned),
                "OSS_CLUSTER_SETTINGS_POLL_MS" => "50",
                "OSS_TAIL_OVERLAY" => return tail.map(|(on, _)| on.to_owned()),
                "OSS_REFRESH_VISIBILITY" => return tail.map(|(_, v)| v.to_owned()),
                _ => return None,
            }
            .to_owned(),
        )
    };
    NodeConfig::from_lookup(lookup).expect("valid")
}

fn call(node: &Node, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(node.addr(), method, target, body.map(str::as_bytes))
}

fn nodes_seen(node: &Node) -> u64 {
    let health = call(node, "GET", "/_cluster/health", None);
    assert_eq!(health.status, 200, "{}", health.text());
    let text = health.text();
    let (_, rest) = text
        .split_once("\"number_of_nodes\":")
        .expect("reports nodes");
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

// docs/specs/writer-hibernation AC-10
#[test]
fn hibernation_is_off_unless_the_variable_sets_an_idle_time() {
    let read = |after: Option<&'static str>| {
        NodeConfig::from_lookup(move |key| match key {
            "OSS_NODE_ID" => Some("n1".to_owned()),
            "OSS_STORE" => Some("fs".to_owned()),
            "OSS_FS_ROOT" => Some("/x".to_owned()),
            "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
            "OSS_HIBERNATE_AFTER_MS" => after.map(str::to_owned),
            _ => None,
        })
    };
    assert_eq!(read(None).expect("valid").hibernate_after_millis, 0);
    assert_eq!(
        read(Some("5000")).expect("valid").hibernate_after_millis,
        5000
    );
    assert!(read(Some("soon")).is_err());
}

// AC-3
#[test]
fn a_started_node_is_enrolled_and_reports_itself() {
    let dir = TempDir::new("boot-health");
    let node = start(&config(dir.path(), "n1", "30000")).expect("starts");
    assert_eq!(nodes_seen(&node), 1);
    assert_eq!(enrolled(dir.path()), ["n1"]);
    let health = call(&node, "GET", "/_cluster/health", None);
    assert!(
        health.text().contains("opensearch-serverless"),
        "{}",
        health.text()
    );
}

// AC-4
#[test]
fn a_started_node_writes_and_reads_a_document() {
    let dir = TempDir::new("boot-doc");
    let node = start(&config(dir.path(), "n1", "30000")).expect("starts");
    let made = call(&node, "PUT", "/logs", None);
    assert_eq!(made.status, 200, "{}", made.text());
    let wrote = call(&node, "PUT", "/logs/_doc/1", Some(r#"{"n":1}"#));
    assert_eq!(wrote.status, 201, "{}", wrote.text());
    assert_eq!(call(&node, "POST", "/logs/_refresh", None).status, 200);
    let read = call(&node, "GET", "/logs/_doc/1", None);
    assert_eq!(read.status, 200, "{}", read.text());
    assert!(read.text().contains("\"n\":1"), "{}", read.text());
}

// AC-5 and AC-6
#[test]
fn a_second_node_is_seen_and_a_dropped_one_is_released() {
    let dir = TempDir::new("boot-pair");
    let first = start(&config(dir.path(), "n1", "3000")).expect("starts");
    let second = start(&config(dir.path(), "n2", "3000")).expect("starts");
    assert_eq!(enrolled(dir.path()), ["n1", "n2"]);
    wait_for("the first never saw the second", || nodes_seen(&first) == 2);
    drop(second);
    // Released, not lapsed: the lease lasts 3 s and the index is read at once.
    assert_eq!(enrolled(dir.path()), ["n1"]);
    // And nothing renews it again: a tick (1 s) passes and it is still gone.
    std::thread::sleep(Duration::from_millis(1_300));
    assert_eq!(enrolled(dir.path()), ["n1"]);
    // The first node's view follows its own refresh tick, which a loaded
    // machine can delay past the sleep above: wait for it, do not assert it.
    wait_for("the first never saw the second leave", || {
        nodes_seen(&first) == 1
    });
}

// AC-8
#[test]
fn a_write_through_one_node_reaches_the_node_that_owns_the_shard() {
    let dir = TempDir::new("boot-forward");
    let first = start(&config(dir.path(), "n1", "30000")).expect("starts");
    let second = start(&config(dir.path(), "n2", "30000")).expect("starts");
    wait_for("the first never saw the second", || nodes_seen(&first) == 2);
    wait_for("the second never saw the first", || {
        nodes_seen(&second) == 2
    });
    let made = call(
        &first,
        "PUT",
        "/fwd",
        Some(r#"{"settings":{"index":{"number_of_shards":8}}}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    // Every id goes in through the second node; the shards it does not own
    // are forwarded to the first.
    for id in 0..24 {
        let wrote = call(
            &second,
            "PUT",
            &format!("/fwd/_doc/{id}?refresh=true"),
            Some(r#"{"n":1}"#),
        );
        assert_eq!(wrote.status, 201, "id {id}: {}", wrote.text());
    }
    // A read through either node finds every document: the shards a node
    // does not hold are asked of their owner.
    for id in 0..24 {
        let target = format!("/fwd/_doc/{id}");
        for node in [&first, &second] {
            let found = call(node, "GET", &target, None);
            assert_eq!(found.status, 200, "id {id}: {}", found.text());
            assert!(found.text().contains(r#""found":true"#), "{}", found.text());
        }
    }
    for node in [&first, &second] {
        let missing = call(node, "GET", "/fwd/_doc/nope", None);
        assert_eq!(missing.status, 404, "{}", missing.text());
    }
    // A refresh through one node reaches the shards the other holds.
    let refresh = call(&first, "POST", "/fwd/_refresh", None);
    assert!(
        refresh.text().contains(r#""failed":0"#),
        "{}",
        refresh.text()
    );
}

// AC-9
#[test]
fn a_crashed_node_leaves_its_lease_to_lapse() {
    let dir = TempDir::new("boot-crash");
    let node = start(&config(dir.path(), "n1", "30000")).expect("starts");
    node.crash();
    // Not released, as a killed process does not: still listed at once.
    assert_eq!(enrolled(dir.path()), ["n1"]);
}

// AC-7
#[test]
fn a_restarted_node_writes_above_the_epochs_it_left() {
    let dir = TempDir::new("boot-restart");
    let write = |node: &Node, index: &str| {
        let r = call(node, "PUT", &format!("/{index}/_doc/1"), Some(r#"{"n":1}"#));
        assert_eq!(r.status, 201, "{}", r.text());
    };
    let first = start(&config(dir.path(), "n1", "30000")).expect("starts");
    assert_eq!(call(&first, "PUT", "/logs", None).status, 200);
    write(&first, "logs");
    let before = epochs(dir.path(), "n1");
    assert!(!before.is_empty(), "the first write opened an epoch");
    drop(first);
    // Released: off the members index, but its register still names the epochs.
    assert_eq!(enrolled(dir.path()), Vec::<String>::new());
    let second = start(&config(dir.path(), "n1", "30000")).expect("starts");
    assert_eq!(
        epochs(dir.path(), "n1"),
        before,
        "enrolling kept the epochs"
    );
    // A shard with a history cannot be taken over yet, so the new process
    // writes a shard of its own.
    assert_eq!(call(&second, "PUT", "/fresh", None).status, 200);
    write(&second, "fresh");
    let after = epochs(dir.path(), "n1");
    assert!(
        before.iter().all(|e| after.contains(e)),
        "{before:?} not all in {after:?}"
    );
    assert!(
        after.iter().max() > before.iter().max(),
        "the new process wrote in a new epoch: {after:?}"
    );
}

/// How many blobs a node's write log holds, over every epoch it names.
fn log_blobs(root: &std::path::Path, id: &str) -> usize {
    fn count(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).map_or(0, |entries| {
            entries
                .flatten()
                .map(|e| {
                    if e.path().is_dir() {
                        count(&e.path())
                    } else {
                        1
                    }
                })
                .sum()
        })
    }
    count(&root.join("nodelog").join(id))
}

// docs/specs/node-dead-log-pass AC-5
#[test]
fn a_booted_node_reaps_the_log_of_a_node_that_died() {
    let dir = TempDir::new("boot-dead-log");
    let first = start(&config(dir.path(), "n1", "3000")).expect("starts");
    assert_eq!(call(&first, "PUT", "/logs", None).status, 200);
    let wrote = call(&first, "PUT", "/logs/_doc/1", Some(r#"{"n":1}"#));
    assert_eq!(wrote.status, 201, "{}", wrote.text());
    first.crash();
    let before = log_blobs(dir.path(), "n1");
    assert!(before > 0, "the first node wrote a log");
    // The second node's pass runs every 100 ms; once the first lease has
    // lapsed it fences the log, which adds a blob to it. Nothing takes the
    // shard over, so the log is still needed and is not deleted.
    let second = start(&config_with(dir.path(), "n2", "3000", Some("100"))).expect("starts");
    wait_for("the dead node's log was never fenced", || {
        log_blobs(dir.path(), "n1") > before
    });
    drop(second);
}

// docs/specs/proactive-takeover AC-2 and AC-1
#[test]
fn a_booted_node_takes_a_dead_nodes_shard_before_any_write_only_when_the_setting_is_on() {
    for on in [false, true] {
        let dir = TempDir::new("boot-proactive");
        let first = start(&config(dir.path(), "n1", "3000")).expect("starts");
        assert_eq!(call(&first, "PUT", "/logs", None).status, 200);
        let wrote = call(&first, "PUT", "/logs/_doc/1", Some(r#"{"n":1}"#));
        assert_eq!(wrote.status, 201, "{}", wrote.text());
        assert_eq!(head_owner(dir.path()).as_deref(), Some("n1"));
        first.crash();
        let second = start(&config_proactive(dir.path(), "n2", "3000", "100", on)).expect("starts");
        if on {
            wait_for("n2 never took the shard", || {
                head_owner(dir.path()).as_deref() == Some("n2")
            });
        } else {
            // Long enough for the lease to lapse and a pass to have run.
            std::thread::sleep(Duration::from_secs(5));
            assert_eq!(
                head_owner(dir.path()).as_deref(),
                Some("n1"),
                "off: untaken"
            );
        }
        drop(second);
    }
}

// docs/specs/node-boot-tail AC-1
#[test]
fn a_node_with_the_tail_overlay_on_starts_and_serves_in_every_visibility() {
    for visibility in ["everywhere", "serving", "owner"] {
        let dir = TempDir::new("boot-tail");
        let node = start(&config_tail(
            dir.path(),
            "n1",
            "30000",
            None,
            Some(("true", visibility)),
        ))
        .expect("starts");
        assert_eq!(nodes_seen(&node), 1, "{visibility}");
        assert!(node.tail_on(), "{visibility}");
        assert_eq!(
            call(&node, "PUT", "/logs", None).status,
            200,
            "{visibility}"
        );
        let wrote = call(&node, "PUT", "/logs/_doc/1", Some(r#"{"n":1}"#));
        assert_eq!(wrote.status, 201, "{visibility}: {}", wrote.text());
        assert_eq!(call(&node, "POST", "/logs/_refresh", None).status, 200);
        let got = call(&node, "GET", "/logs/_doc/1", None);
        assert_eq!(got.status, 200, "{visibility}: {}", got.text());
    }
}

// docs/specs/node-boot-tail AC-2
#[test]
fn a_node_starts_with_the_tail_overlay_off_unless_the_setting_is_on() {
    let dir = TempDir::new("boot-tail-off");
    let node = start(&config(dir.path(), "n1", "30000")).expect("starts");
    assert!(!node.tail_on());
    let dir = TempDir::new("boot-tail-false");
    let node = start(&config_tail(
        dir.path(),
        "n1",
        "30000",
        None,
        Some(("false", "everywhere")),
    ))
    .expect("starts");
    assert!(!node.tail_on());
}

// docs/specs/node-cluster-visibility AC-6 and AC-7
#[test]
fn a_booted_node_follows_the_cluster_registers_refresh_visibility() {
    use oss_node::writer::RefreshVisibility::{Everywhere, Owner, Serving};
    let dir = TempDir::new("boot-follow");
    let node = start(&config_tail(
        dir.path(),
        "n1",
        "30000",
        None,
        Some(("true", "owner")),
    ))
    .expect("starts");
    assert_eq!(node.refresh_visibility(), Some(Owner));
    let put = |value: &str| {
        let body =
            format!(r#"{{"persistent":{{"serverless.search.refresh_visibility":{value}}}}}"#);
        let r = call(&node, "PUT", "/_cluster/settings", Some(&body));
        assert_eq!(r.status, 200, "{}", r.text());
    };
    put(r#""everywhere""#);
    wait_for("the cluster's mode is in force", || {
        node.refresh_visibility() == Some(Everywhere)
    });
    put(r#""serving""#);
    wait_for("the next mode is in force", || {
        node.refresh_visibility() == Some(Serving)
    });
    put("null");
    wait_for("removing it returns to the node's", || {
        node.refresh_visibility() == Some(Owner)
    });
    // Off, there is nothing to follow and nothing is read.
    let dir = TempDir::new("boot-follow-off");
    let node = start(&config(dir.path(), "n1", "30000")).expect("starts");
    assert_eq!(node.refresh_visibility(), None);
}

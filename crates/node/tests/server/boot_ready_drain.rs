//! docs/specs/node-ready-drain: a started node says whether it is ready, and
//! a drain takes the search role out of its lease.

use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_blob::FsStore;
use oss_cluster::{LeaseClock, LeaseConfig, Membership, SystemClock};
use oss_node::boot::{NodeConfig, start};
use oss_testkit::TempDir;

use crate::support;

/// The settle the test sets, in milliseconds, against a lease TTL of 300.
const SETTLE_MS: u64 = 1000;

fn lookup<'a>(
    root: &'a str,
    extra: &'a [(&'a str, &'a str)],
) -> impl Fn(&str) -> Option<String> + 'a {
    move |key| {
        if let Some((_, value)) = extra.iter().find(|(k, _)| *k == key) {
            return Some((*value).to_owned());
        }
        Some(
            match key {
                "OSS_NODE_ID" => "n1",
                "OSS_STORE" => "fs",
                "OSS_FS_ROOT" => root,
                "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => "127.0.0.1:0",
                "OSS_LEASE_TTL_MS" => "300",
                _ => return None,
            }
            .to_owned(),
        )
    }
}

fn observer(root: &std::path::Path) -> Membership {
    let clock: Arc<dyn LeaseClock> = Arc::new(SystemClock);
    Membership::new(
        Arc::new(FsStore::new(root).expect("store")),
        "",
        clock,
        LeaseConfig::default(),
    )
    .expect("membership")
}

/// The roles `n1`'s lease names, as a peer reads them.
fn roles(observer: &Membership) -> Vec<String> {
    observer.refresh().expect("reads");
    observer
        .current()
        .iter()
        .find(|l| l.node_id == "n1")
        .map(|l| l.roles().to_vec())
        .unwrap_or_default()
}

// docs/specs/node-ready-drain AC-1, AC-2, AC-3, AC-5
#[test]
fn a_drain_makes_the_node_not_ready_and_takes_the_search_role_from_its_lease() {
    let dir = TempDir::new("boot-ready-drain");
    let root = dir.path().display().to_string();
    let config =
        NodeConfig::from_lookup(lookup(&root, &[("OSS_DRAIN_SETTLE_MS", "1000")])).expect("valid");
    let node = start(&config).expect("starts");
    let watcher = observer(dir.path());
    assert_eq!(roles(&watcher), ["ingest", "search"]);

    let ready = support::request(node.addr(), "GET", "/_serverless/ready", None);
    assert_eq!(ready.status, 200, "{}", ready.text());
    assert!(ready.text().contains(r#""ready":true"#), "{}", ready.text());

    // The drain is under way on another thread while the probe is asked: the
    // probe answers not ready before the settle is over.
    let addr = node.addr();
    let asked = Instant::now();
    let drainer =
        std::thread::spawn(move || support::request(addr, "POST", "/_serverless/drain", None));
    let started = Instant::now();
    let not_ready = loop {
        let reply = support::request(addr, "GET", "/_serverless/ready", None);
        if reply.status == 503 {
            break reply;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "never not ready"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        asked.elapsed() < Duration::from_millis(SETTLE_MS),
        "answered only after the wait"
    );
    assert!(
        not_ready.text().contains(r#""state":"draining""#),
        "{}",
        not_ready.text()
    );
    let drained = drainer.join().expect("drain thread");
    assert_eq!(drained.status, 200, "{}", drained.text());
    assert!(
        drained.text().contains(r#""drained":true"#),
        "{}",
        drained.text()
    );
    let took: u64 = drained
        .text()
        .split(r#""took_millis":"#)
        .nth(1)
        .and_then(|rest| rest.trim_end_matches(['}', ' ']).parse().ok())
        .expect("took_millis");
    assert!(
        took >= SETTLE_MS,
        "answered after {took} ms, before the settle"
    );

    // The role is gone from the lease by the time the drain answered.
    assert_eq!(roles(&watcher), ["ingest"]);

    // A second call does nothing and does not wait.
    let again = Instant::now();
    let second = support::request(node.addr(), "POST", "/_serverless/drain", None);
    assert!(
        second.text().contains(r#""already":true"#),
        "{}",
        second.text()
    );
    assert!(
        again.elapsed() < Duration::from_millis(SETTLE_MS / 4),
        "waited"
    );

    // The node still owns its shards and takes writes.
    let made = support::request(node.addr(), "PUT", "/logs", Some(b"{}"));
    assert_eq!(made.status, 200, "{}", made.text());
    let wrote = support::request(
        node.addr(),
        "PUT",
        "/logs/_doc/1?refresh=true",
        Some(br#"{"a":1}"#),
    );
    assert!(wrote.status < 300, "{}", wrote.text());
}

// docs/specs/node-ready-drain AC-6
#[test]
fn a_drain_wait_over_a_minute_is_refused_at_start() {
    let dir = TempDir::new("boot-drain-limit");
    let root = dir.path().display().to_string();
    let too_long = [("OSS_DRAIN_SETTLE_MS", "60001")];
    assert!(NodeConfig::from_lookup(lookup(&root, &too_long)).is_err());
    let fine = [("OSS_DRAIN_SETTLE_MS", "60000")];
    assert!(NodeConfig::from_lookup(lookup(&root, &fine)).is_ok());
}

//! docs/specs/frozen-node-suite AC-6: a shard owner that is a real process
//! stopped with SIGSTOP, so its threads and timers stop too, loses its shard
//! to a survivor and, resumed, loses and doubles nothing it acknowledged.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator", unix))]

mod suite_rig;

use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use suite_rig::{Cluster, Reply, wait_for};

const TTL_MS: u64 = 3_000;

/// A node that is its own process; resumed and killed when dropped, so a
/// stopped child is not left behind.
struct Proc {
    child: Child,
    http: SocketAddr,
}

impl Proc {
    fn signal(&self, name: &str) {
        let status = Command::new("kill")
            .arg(format!("-{name}"))
            .arg(self.child.id().to_string())
            .status()
            .expect("kill runs");
        assert!(status.success(), "kill -{name} failed");
    }

    fn call(&self, method: &str, target: &str, body: Option<&str>) -> Reply {
        suite_rig::request(self.http, method, target, body.map(str::as_bytes))
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .arg("-CONT")
            .arg(self.child.id().to_string())
            .status();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("a port")
        .local_addr()
        .expect("an address")
        .port()
}

fn spawn(cluster: &Cluster, id: &str) -> Proc {
    let (endpoint, bucket, region, access, secret) = cluster.store();
    let http: SocketAddr = format!("127.0.0.1:{}", free_port()).parse().expect("addr");
    let child = Command::new(env!("CARGO_BIN_EXE_oss-node"))
        .env("OSS_NODE_ID", id)
        .env("OSS_STORE", "s3")
        .env("OSS_S3_ENDPOINT", endpoint)
        .env("OSS_S3_BUCKET", bucket)
        .env("OSS_S3_REGION", region)
        .env("OSS_S3_ACCESS_KEY", access)
        .env("OSS_S3_SECRET_KEY", secret)
        .env("OSS_HTTP_ADDR", http.to_string())
        .env("OSS_TRANSPORT_ADDR", format!("127.0.0.1:{}", free_port()))
        .env("OSS_LEASE_TTL_MS", TTL_MS.to_string())
        .env("OSS_MEMORY_BYTES", "8589934592")
        .env("OSS_TAIL_OVERLAY", "true")
        .env("OSS_REFRESH_VISIBILITY", "everywhere")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the node binary starts");
    let proc = Proc { child, http };
    wait_for("the process answers", 30, || {
        std::net::TcpStream::connect(http).is_ok()
            && proc.call("GET", "/_cluster/health", None).status == 200
    });
    proc
}

fn body(ids: &[String], n: u32) -> String {
    ids.iter()
        .map(|id| {
            format!("{{\"index\":{{\"_index\":\"logs\",\"_id\":\"{id}\"}}}}\n{{\"n\":{n}}}\n")
        })
        .collect()
}

fn ids(range: std::ops::Range<u32>) -> Vec<String> {
    range.map(|i| format!("d{i}")).collect()
}

/// Sends `body` through `send` until every item is created or 60 s pass.
fn until_created(send: impl Fn(&str) -> Reply, body: &str) {
    let started = Instant::now();
    loop {
        let r = send(body);
        if r.status == 200 && r.text().contains("\"errors\":false") {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the bulk never succeeded: {}",
            r.text()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn total(cluster: &Cluster, slot: usize, value: Option<u32>) -> Option<u64> {
    let query = value.map_or_else(
        || r#"{"match_all":{}}"#.to_owned(),
        |v| format!(r#"{{"term":{{"n":{v}}}}}"#),
    );
    let request = format!(r#"{{"query":{query},"size":0,"track_total_hits":true}}"#);
    let r = cluster.call(slot, "POST", "/logs/_search", Some(&request));
    let text = r.text();
    if r.status != 200 || !text.contains("\"failed\":0") {
        return None;
    }
    let (_, rest) = text.split_once("\"value\":")?;
    rest.chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

/// Polls until `slot` counts `want`, failing if a poll ever counts more.
fn converges(cluster: &Cluster, slot: usize, value: Option<u32>, want: u64) {
    let started = Instant::now();
    loop {
        if let Some(t) = total(cluster, slot, value) {
            assert!(t <= want, "node {slot} counted {t}, more than {want}");
            if t == want {
                return;
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "node {slot} never counted {want} for {value:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_process_stopped_with_sigstop_loses_its_shard_and_doubles_nothing_on_resume() {
    let mut cluster = Cluster::new(TTL_MS, 4, false).expect("emulated");
    cluster.set("OSS_TAIL_OVERLAY", "true");
    cluster.set("OSS_REFRESH_VISIBILITY", "everywhere");
    // The process starts alone so it is the one that takes the shard.
    let owner = spawn(&cluster, "p1");
    std::thread::sleep(Duration::from_millis(TTL_MS));
    let made = owner.call(
        "PUT",
        "/logs",
        Some(r#"{"settings":{"index":{"number_of_shards":1}},"mappings":{"properties":{"n":{"type":"integer"}}}}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    until_created(
        |b| owner.call("POST", "/_bulk?refresh=true", Some(b)),
        &body(&ids(0..20), 1),
    );
    assert_eq!(cluster.owner_id("logs", 0).as_deref(), Some("p1"));

    let s0 = cluster.add("n1");
    let s1 = cluster.add("n2");
    wait_for("the survivors see three", 30, || {
        cluster.nodes_seen(s0) == 3 && cluster.nodes_seen(s1) == 3
    });
    for slot in [s0, s1] {
        converges(&cluster, slot, None, 20);
    }

    owner.signal("STOP");
    let held_body = body(&ids(100..103), 9);
    let reply = std::thread::scope(|scope| {
        // Sent to a stopped process: it waits in the socket until the resume.
        let sender = scope.spawn(|| owner.call("POST", "/_bulk?refresh=true", Some(&held_body)));
        wait_for("the survivors never saw the process go", 25, || {
            cluster.nodes_seen(s0) == 2 && cluster.nodes_seen(s1) == 2
        });
        std::thread::sleep(Duration::from_millis(TTL_MS * 2));
        until_created(
            |b| cluster.call(s0, "POST", "/_bulk?refresh=true", Some(b)),
            &body(&ids(20..25), 1),
        );
        for slot in [s0, s1] {
            converges(&cluster, slot, None, 25);
        }
        let head = cluster.owner_id("logs", 0).expect("a head owner");
        assert!(
            head == "n1" || head == "n2",
            "the head names {head}, not a survivor"
        );
        owner.signal("CONT");
        sender.join().expect("the sender finishes")
    });
    let successor = cluster.owner_id("logs", 0).expect("a head owner");

    // The held write was acknowledged whole or refused; nothing in between
    // is counted.
    let acknowledged = reply.status == 200 && reply.text().contains("\"errors\":false");
    let held_counted = if acknowledged { 3 } else { 0 };
    // The resumed process serves a write for the shard.
    until_created(
        |b| owner.call("POST", "/_bulk?refresh=true", Some(b)),
        &body(&ids(25..26), 1),
    );
    for slot in [s0, s1] {
        converges(&cluster, slot, None, 26 + held_counted);
        converges(&cluster, slot, Some(9), held_counted);
    }
    // The head is held by one node: the survivor that took it, before and
    // after the resume.
    let after = cluster.owner_id("logs", 0).expect("a head owner");
    assert_eq!(after, successor, "the head moved after the resume");
    assert_ne!(after, "p1", "the resumed process took the head back");
}

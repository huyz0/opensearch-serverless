//! docs/specs/mixed-fleet-soak: Java shell and Rust nodes as live processes on one RustFS
//! bucket, handing a shard over by kill, judged by a ledger of every acknowledged document.
//! Ignored unless `RUSTFS_BIN`, `OSS_JAVA_BIN` and `OSS_JAVA_CP` are set.
#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: batch sizes, chunk offsets and durations are small and bounded by the reply read.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "mixed_fleet/made_from_nothing.rs"]
mod made_from_nothing;
#[path = "mixed_fleet/snapshots.rs"]
mod snapshots;
#[path = "mixed_fleet/soak.rs"]
mod soak;
#[path = "mixed_fleet/soak_wide.rs"]
mod soak_wide;
mod suite_rig;

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use suite_rig::Cluster;

const TTL_MS: u64 = 4_000;
/// What a hand-over may take beyond the lease: the successor sees the lease
/// lapse on its next poll, then opens the shard.
const HANDOVER_SLACK_SECS: u64 = 60;
const BATCH: u32 = 100;
const INDEX: &str = "mixed";
/// The most documents one match-all returns: the default result window.
const MAX_FOUND: u32 = 10_000;

/// A forked Java shell node.
struct JavaNode {
    child: Child,
    addr: SocketAddr,
}

impl JavaNode {
    fn start(name: &str, home: &Path, cluster: &Cluster, buckets: bool) -> Self {
        let java = std::env::var("OSS_JAVA_BIN").expect("OSS_JAVA_BIN");
        let cp = std::env::var("OSS_JAVA_CP").expect("OSS_JAVA_CP");
        let (endpoint, bucket, region, access, secret) = cluster.store();
        let keystore = home.join("config/opensearch.keystore");
        if !keystore.exists() {
            let status = Command::new(&java)
                .args(["-cp", &cp])
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../fixtures/mixed-fleet/WriteKeystore.java"),
                )
                .arg(home)
                .args([access, secret])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("the keystore helper runs");
            assert!(status.success(), "the keystore helper");
        }
        let ready = home.join("ready");
        let _ = std::fs::remove_file(&ready);
        let log = std::fs::File::create(home.join("node.log")).expect("a log");
        let mut command = Command::new(&java);
        command
            .args([
                "-cp",
                &cp,
                "--add-opens=java.base/java.lang=ALL-UNNAMED",
                "--add-opens=java.base/java.nio=ALL-UNNAMED",
                "--add-opens=java.base/java.util=ALL-UNNAMED",
                "-Dio.netty.noUnsafe=true",
                "-Dio.netty.noKeySetOptimization=true",
                "-Dio.netty.recycler.maxCapacityPerThread=0",
                "-Dopensearch.set.netty.runtime.available.processors=false",
            ])
            .arg(format!(
                "-Dopensearch.path.conf={}",
                home.join("config").display()
            ));
        let settings = [
            ("node.name", name.to_owned()),
            ("cluster.name", "mixed-fleet".to_owned()),
            ("path.home", home.display().to_string()),
            ("path.conf", home.join("config").display().to_string()),
            ("network.host", "127.0.0.1".to_owned()),
            ("http.port", "0".to_owned()),
            ("transport.port", "0".to_owned()),
            ("serverless.store.type", "s3".to_owned()),
            ("serverless.store.bucket", bucket.to_owned()),
            ("serverless.store.endpoint", endpoint),
            ("serverless.store.region", region.to_owned()),
            ("serverless.store.path_style_access", "true".to_owned()),
            ("serverless.store.provider_ack", "true".to_owned()),
            ("serverless.lease.ttl_millis", TTL_MS.to_string()),
            ("serverless.ready.file", ready.display().to_string()),
        ];
        for (k, v) in settings {
            command.arg(format!("-D{k}={v}"));
        }
        if buckets {
            command.arg("-Dserverless.buckets.enabled=true");
        }
        command
            .arg("org.opensearch.serverless.shell.ServerlessBootstrap")
            .stdout(log.try_clone().expect("a log handle"))
            .stderr(log);
        let mut child = command.spawn().expect("the Java node forks");
        let started = Instant::now();
        let addr = loop {
            // The file may be read between its creation and its content.
            if let Ok(line) = std::fs::read_to_string(&ready)
                && let Some(http) = line
                    .split_whitespace()
                    .find_map(|t| t.strip_prefix("http="))
                && let Ok(addr) = http.parse()
            {
                break addr;
            }
            assert!(
                child.try_wait().expect("polls").is_none(),
                "the Java node exited; see {}",
                home.join("node.log").display()
            );
            assert!(
                started.elapsed() < Duration::from_secs(180),
                "the Java node did not start"
            );
            std::thread::sleep(Duration::from_millis(200));
        };
        Self { child, addr }
    }

    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for JavaNode {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One REST call, read to the end of its framing, with the connection kept
/// open: asked to `connection: close`, the Java shell sends the head of a
/// chunked bulk reply and closes without the body.
fn try_call(addr: SocketAddr, method: &str, target: &str, body: Option<&str>) -> Option<Reply> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(addr).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(60))).ok()?;
    let body = body.unwrap_or("");
    let head = format!(
        "{method} {target} HTTP/1.1\r\nhost: test\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).ok()?;
    s.write_all(body.as_bytes()).ok()?;
    let mut raw = Vec::new();
    let mut buf = [0u8; 16 << 10];
    let framed = |raw: &[u8]| -> Option<Reply> {
        let end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
        let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
        let status: u16 = head.split(' ').nth(1)?.parse().ok()?;
        let rest = &raw[end + 4..];
        let text = if head.contains("transfer-encoding: chunked") {
            let mut out = Vec::new();
            let mut at = rest;
            loop {
                let line = at.windows(2).position(|w| w == b"\r\n")?;
                let size = usize::from_str_radix(std::str::from_utf8(&at[..line]).ok()?.trim(), 16)
                    .ok()?;
                at = &at[line + 2..];
                if size == 0 {
                    break;
                }
                if at.len() < size + 2 {
                    return None;
                }
                out.extend_from_slice(&at[..size]);
                at = &at[size + 2..];
            }
            out
        } else {
            let len = head
                .split("content-length:")
                .nth(1)
                .and_then(|r| r.split("\r\n").next())
                .and_then(|n| n.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if rest.len() < len {
                return None;
            }
            rest[..len].to_vec()
        };
        Some(Reply {
            status,
            text: String::from_utf8_lossy(&text).into_owned(),
        })
    };
    loop {
        if let Some(reply) = framed(&raw) {
            return Some(reply);
        }
        match s.read(&mut buf) {
            Ok(0) | Err(_) => return None,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
        }
    }
}

/// [`try_call`] for a node that must answer.
fn call(addr: SocketAddr, method: &str, target: &str, body: Option<&str>) -> Reply {
    try_call(addr, method, target, body)
        .unwrap_or_else(|| panic!("{method} {target} on {addr} got no complete reply"))
}

struct Reply {
    status: u16,
    text: String,
}

impl Reply {
    fn text(&self) -> String {
        self.text.clone()
    }
}

/// The documents `ids` of `INDEX`, acknowledged by the node at `addr`; a
/// refusal during a hand-over is retried for `HANDOVER_SLACK_SECS` past the
/// lease. Returns the ids acknowledged, which the ledger then holds.
fn write(addr: SocketAddr, ids: std::ops::Range<u32>, ledger: &mut BTreeSet<u32>) {
    let mut body = String::new();
    for id in ids.clone() {
        body.push_str(&format!(
            "{{\"index\":{{\"_index\":\"{INDEX}\",\"_id\":\"{id}\"}}}}\n{{\"n\":{id}}}\n"
        ));
    }
    let started = Instant::now();
    loop {
        let r = call(addr, "POST", "/_bulk?refresh=true", Some(&body));
        let text = r.text();
        if r.status == 200 && text.contains("\"errors\":false") {
            eprintln!(
                "acknowledged {} documents after {} ms",
                ids.len(),
                started.elapsed().as_millis()
            );
            ledger.extend(ids);
            return;
        }
        assert!(
            started.elapsed()
                < Duration::from_millis(TTL_MS) + Duration::from_secs(HANDOVER_SLACK_SECS),
            "no acknowledgement within the lease plus {HANDOVER_SLACK_SECS} s: {} {text}",
            r.status
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Every `_id` the node at `addr` returns for a match-all, with duplicates kept.
fn found(addr: SocketAddr) -> Vec<u32> {
    let body = format!(
        "{{\"size\":{},\"_source\":false,\"query\":{{\"match_all\":{{}}}}}}",
        MAX_FOUND
    );
    let r = call(addr, "POST", &format!("/{INDEX}/_search"), Some(&body));
    assert_eq!(r.status, 200, "{}", r.text());
    let text = r.text();
    text.split("\"_id\":")
        .skip(1)
        .map(|rest| {
            rest.trim_start()
                .trim_start_matches('"')
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .expect("a numeric id")
        })
        .collect()
}

/// Fails, naming the ids, unless the node at `addr` finds exactly the ledger.
fn judge(step: &str, addr: SocketAddr, ledger: &BTreeSet<u32>) {
    let started = Instant::now();
    loop {
        let _ = call(addr, "POST", &format!("/{INDEX}/_refresh"), None);
        let ids = found(addr);
        let set: BTreeSet<u32> = ids.iter().copied().collect();
        let missing: Vec<_> = ledger.difference(&set).take(10).collect();
        let extra: Vec<_> = set.difference(ledger).take(10).collect();
        if missing.is_empty() && extra.is_empty() && ids.len() == ledger.len() {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(HANDOVER_SLACK_SECS),
            "{step}: ledger {} found {} (missing {missing:?}, unexpected {extra:?})",
            ledger.len(),
            ids.len()
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("oss-mixed-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory");
    dir
}

#[test]
#[ignore = "needs RUSTFS_BIN, OSS_JAVA_BIN and OSS_JAVA_CP"]
fn a_shard_changes_hands_between_java_and_rust_without_losing_a_write() {
    hand_over(false);
}

#[test]
#[ignore = "needs RUSTFS_BIN, OSS_JAVA_BIN and OSS_JAVA_CP"]
fn a_bucketed_index_changes_hands_between_java_and_rust_without_losing_a_write() {
    hand_over(true);
}

/// The hand-over scenario; `buckets` turns the switch on in every node and
/// makes the index one shard, so it lives in a bucket.
fn hand_over(buckets: bool) {
    if std::env::var_os("OSS_JAVA_CP").is_none() || std::env::var_os("OSS_JAVA_BIN").is_none() {
        eprintln!("skipped: OSS_JAVA_BIN and OSS_JAVA_CP are not set");
        return;
    }
    let Some(mut cluster) = Cluster::new(TTL_MS, 1, true) else {
        eprintln!("skipped: RUSTFS_BIN is not set");
        return;
    };
    if buckets {
        cluster.set("OSS_BUCKETS_ENABLED", "true");
    }
    let shards = if buckets { 1 } else { 2 };
    let mut ledger = BTreeSet::new();

    // AC-1: Java creates and writes; Rust, started after, finds it all.
    let j1 = JavaNode::start("java-1", &temp("j1"), &cluster, buckets);
    let made = call(
        j1.addr,
        "PUT",
        &format!("/{INDEX}"),
        Some(&format!(
            "{{\"settings\":{{\"index\":{{\"number_of_shards\":{shards}}}}}}}"
        )),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    write(j1.addr, 0..BATCH, &mut ledger);
    judge("AC-1 java reads its own batch", j1.addr, &ledger);
    let r1 = cluster.add("rust-1");
    let r1_addr = cluster.node(r1).addr();
    judge("AC-1 rust finds java's batch", r1_addr, &ledger);

    // AC-2: Java is killed; Rust takes over and writes.
    j1.kill();
    write(r1_addr, BATCH..2 * BATCH, &mut ledger);
    judge("AC-2 rust after taking over", r1_addr, &ledger);

    // AC-3: a second Java node finds both batches.
    let j2 = JavaNode::start("java-2", &temp("j2"), &cluster, buckets);
    judge("AC-3 second java", j2.addr, &ledger);

    // AC-4: Rust is killed; the second Java node takes over and writes.
    cluster.kill(r1);
    write(j2.addr, 2 * BATCH..3 * BATCH, &mut ledger);
    let r2 = cluster.add("rust-2");
    let r2_addr = cluster.node(r2).addr();
    judge("AC-4 second rust", r2_addr, &ledger);
    judge("AC-4 java", j2.addr, &ledger);
    j2.kill();
}

/// A TCP relay to `target` that counts the connections it takes, so a test
/// can tell a request the Java node served from one a Rust node answered.
fn counting_relay(
    target: SocketAddr,
) -> (SocketAddr, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("binds");
    let addr = listener.local_addr().expect("an address");
    let count = std::sync::Arc::new(AtomicUsize::new(0));
    let seen = std::sync::Arc::clone(&count);
    std::thread::spawn(move || {
        for client in listener.incoming() {
            let Ok(client) = client else { return };
            seen.fetch_add(1, Ordering::SeqCst);
            let Ok(server) = std::net::TcpStream::connect(target) else {
                continue;
            };
            for (mut from, mut to) in [
                (
                    client.try_clone().expect("clone"),
                    server.try_clone().expect("clone"),
                ),
                (server, client),
            ] {
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut from, &mut to);
                    let _ = to.shutdown(std::net::Shutdown::Write);
                });
            }
        }
    });
    (addr, count)
}

/// A plugin request through a Rust node reaches the Java node behind it.
///
/// The shell here runs no plugin, so it answers a plugin path with the same
/// "no handler found" a Rust node would; a relay in front of the Java node
/// counts the connections that reach it, which tells who answered. This runs
/// on the emulator's S3 front end: nothing here needs RustFS.
#[test]
#[ignore = "needs OSS_JAVA_BIN and OSS_JAVA_CP"]
fn a_plugin_request_through_a_rust_node_is_answered_by_the_java_node() {
    use std::sync::atomic::Ordering;
    if std::env::var_os("OSS_JAVA_CP").is_none() || std::env::var_os("OSS_JAVA_BIN").is_none() {
        eprintln!("skipped: OSS_JAVA_BIN and OSS_JAVA_CP are not set");
        return;
    }
    let mut cluster = Cluster::new(TTL_MS, 1, false).expect("the emulated cluster");
    let java = JavaNode::start("java-p", &temp("jp"), &cluster, false);
    let (relay, reached) = counting_relay(java.addr);
    let target = "/_plugins/_ism/policies?probe=1";

    // AC-1: with no upstream a Rust node answers the plugin path itself.
    let plain = cluster.add("rust-plain");
    let own = cluster.call(plain, "GET", target, None);
    assert_eq!(own.status, 400, "{}", own.text());
    assert!(own.text().contains("no handler found"), "{}", own.text());
    assert_eq!(reached.load(Ordering::SeqCst), 0);

    // AC-2: with the Java node as upstream, the Java node answers.
    cluster.set("OSS_PLUGIN_UPSTREAMS", &format!("http://{relay}"));
    let proxied = cluster.add("rust-proxy");
    let direct = call(java.addr, "GET", target, None);
    let through = cluster.call(proxied, "GET", target, None);
    assert_eq!(
        (through.status, through.text()),
        (direct.status, direct.text())
    );
    assert_eq!(
        reached.load(Ordering::SeqCst),
        1,
        "one request reached the Java node"
    );

    // AC-3: a native route on the proxying node is still the Rust node's.
    let health = cluster.call(proxied, "GET", "/_cluster/health", None);
    assert_eq!(health.status, 200, "{}", health.text());
    assert_eq!(
        reached.load(Ordering::SeqCst),
        1,
        "a native route is not proxied"
    );

    // AC-4: a write through the proxy reaches the Java node, answered as it answers.
    let body = "{\"x\":1}";
    let path = "/_plugins/_ml/models/_search";
    let direct = call(java.addr, "POST", path, Some(body));
    let through = cluster.call(proxied, "POST", path, Some(body));
    assert_eq!(
        (through.status, through.text()),
        (direct.status, direct.text())
    );
    assert_eq!(reached.load(Ordering::SeqCst), 2);
    java.kill();
}

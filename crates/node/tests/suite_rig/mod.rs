//! A rig for the multi-node suite: three real nodes on one object store
//! behind an S3 HTTP front end with the request latency the Java shell's
//! test kit injects, or on a RustFS process when `RUSTFS_BIN` names one.

// Each test file uses part of the rig.
#![allow(dead_code)]

#[path = "../support/mod.rs"]
mod support;

use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use oss_blob::emulator::faults::Latency;
use oss_blob::emulator::http::S3Server;
use oss_blob::s3::sigv4::{self, Credentials, Scope, UNSIGNED_PAYLOAD};
use oss_blob::{Emulator, S3Op};
use oss_node::boot::{Node, NodeConfig, start};

pub use support::Reply;

const ACCESS: &str = "suiteaccess";
const SECRET: &str = "suitesecret";
const BUCKET: &str = "suite";
const REGION: &str = "us-east-1";

/// The store the nodes share.
enum Backing {
    /// The in-process emulator behind its S3 front end, with latency.
    Emulated(S3Server),
    /// A RustFS process; killed when the cluster is dropped.
    RustFs(Child, std::path::PathBuf),
}

impl Drop for Backing {
    fn drop(&mut self) {
        if let Self::RustFs(child, dir) = self {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Nodes sharing one store; a slot is empty once its node is stopped.
pub struct Cluster {
    endpoint: String,
    ttl: u64,
    nodes: Vec<Option<Node>>,
    _backing: Backing,
}

/// The latency profile of the Java shell's test kit: GET 20-30 ms, PUT
/// 40-60 ms, LIST 40-70 ms, scaled down by `divisor` to keep a run short.
fn profile(emulator: &Emulator, divisor: u32) {
    let ms = |base: u64, jitter: u64| Latency {
        base: Duration::from_millis(base)
            .checked_div(divisor)
            .unwrap_or_default(),
        jitter: Duration::from_millis(jitter)
            .checked_div(divisor)
            .unwrap_or_default(),
    };
    emulator.set_latency(S3Op::Get, ms(20, 10));
    emulator.set_latency(S3Op::GetRange, ms(20, 10));
    emulator.set_latency(S3Op::Head, ms(20, 10));
    emulator.set_latency(S3Op::Put, ms(40, 20));
    emulator.set_latency(S3Op::List, ms(40, 30));
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a port")
        .local_addr()
        .expect("an address")
        .port()
}

fn create_bucket(endpoint: &str, host: &str) {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs();
    let date = sigv4::amz_date(now);
    let headers = [
        ("host", host),
        ("x-amz-content-sha256", UNSIGNED_PAYLOAD),
        ("x-amz-date", date.as_str()),
    ];
    let path = format!("/{BUCKET}");
    let canonical = sigv4::canonical_request("PUT", &path, "", &headers, UNSIGNED_PAYLOAD);
    let scope = Scope {
        date: &date[..8],
        region: REGION,
        service: "s3",
    };
    let auth = sigv4::authorization(&Credentials::new(ACCESS, SECRET), &scope, &date, &canonical);
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .proxy(None)
            .build(),
    );
    let response = agent
        .put(&format!("{endpoint}{path}"))
        .header("x-amz-content-sha256", UNSIGNED_PAYLOAD)
        .header("x-amz-date", &date)
        .header("authorization", &auth)
        .send_empty()
        .expect("the bucket request");
    assert_eq!(response.status().as_u16(), 200, "create bucket");
}

impl Cluster {
    /// An empty cluster over the emulator with latency (`divisor` 1 is
    /// the Java profile), or over RustFS when `RUSTFS_BIN` is set and
    /// `rustfs` asks for it. Leases last `ttl_ms`.
    pub fn new(ttl_ms: u64, divisor: u32, rustfs: bool) -> Option<Self> {
        if rustfs {
            let bin = std::env::var_os("RUSTFS_BIN")?;
            let dir = std::env::temp_dir().join(format!("oss-suite-rustfs-{}", free_port()));
            std::fs::create_dir_all(&dir).expect("a directory");
            let host = format!("127.0.0.1:{}", free_port());
            let child = Command::new(bin)
                .arg(&dir)
                .env("RUSTFS_ADDRESS", &host)
                .env("RUSTFS_VOLUMES", &dir)
                .env("RUSTFS_ACCESS_KEY", ACCESS)
                .env("RUSTFS_SECRET_KEY", SECRET)
                .env("RUSTFS_CONSOLE_ENABLE", "false")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("RUSTFS_BIN starts");
            let backing = Backing::RustFs(child, dir);
            let started = Instant::now();
            while TcpStream::connect(&host).is_err() {
                assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "RustFS did not listen"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
            let endpoint = format!("http://{host}");
            create_bucket(&endpoint, &host);
            return Some(Self {
                endpoint,
                ttl: ttl_ms,
                nodes: Vec::new(),
                _backing: backing,
            });
        }
        let emulator = Arc::new(Emulator::new(1 << 30, 7));
        profile(&emulator, divisor);
        let server = S3Server::start(emulator, BUCKET, REGION, Credentials::new(ACCESS, SECRET))
            .expect("the S3 front end");
        Some(Self {
            endpoint: server.endpoint(),
            ttl: ttl_ms,
            nodes: Vec::new(),
            _backing: Backing::Emulated(server),
        })
    }

    fn config(&self, id: &str) -> NodeConfig {
        let ttl = self.ttl.to_string();
        let lookup = |key: &str| {
            Some(
                match key {
                    "OSS_NODE_ID" => id,
                    "OSS_STORE" => "s3",
                    "OSS_S3_ENDPOINT" => self.endpoint.as_str(),
                    "OSS_S3_BUCKET" => BUCKET,
                    "OSS_S3_REGION" => REGION,
                    "OSS_S3_ACCESS_KEY" => ACCESS,
                    "OSS_S3_SECRET_KEY" => SECRET,
                    "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => "127.0.0.1:0",
                    "OSS_LEASE_TTL_MS" => ttl.as_str(),
                    "OSS_MEMORY_BYTES" => "8589934592",
                    _ => return None,
                }
                .to_owned(),
            )
        };
        NodeConfig::from_lookup(lookup).expect("a valid configuration")
    }

    /// Starts the node `id` and returns its slot.
    pub fn add(&mut self, id: &str) -> usize {
        let node = start(&self.config(id)).expect("the node starts");
        self.nodes.push(Some(node));
        self.nodes.len().saturating_sub(1)
    }

    /// The node in `slot`.
    pub fn node(&self, slot: usize) -> &Node {
        self.nodes[slot].as_ref().expect("the node is running")
    }

    /// The slots whose node runs.
    pub fn live(&self) -> Vec<usize> {
        (0..self.nodes.len())
            .filter(|i| self.nodes[*i].is_some())
            .collect()
    }

    /// Stops the node in `slot` as a killed process stops.
    pub fn kill(&mut self, slot: usize) {
        self.nodes[slot]
            .take()
            .expect("the node is running")
            .crash();
    }

    /// One REST call to the node in `slot`.
    pub fn call(&self, slot: usize, method: &str, target: &str, body: Option<&str>) -> Reply {
        support::request(
            self.node(slot).addr(),
            method,
            target,
            body.map(str::as_bytes),
        )
    }

    /// The nodes the node in `slot` counts in its cluster health.
    pub fn nodes_seen(&self, slot: usize) -> u64 {
        let health = self.call(slot, "GET", "/_cluster/health", None);
        assert_eq!(health.status, 200, "{}", health.text());
        let text = health.text();
        let (_, rest) = text
            .split_once("\"number_of_nodes\":")
            .expect("health reports nodes");
        rest.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .expect("a number")
    }
}

/// Waits until `done`, failing with `what` after `secs`.
pub fn wait_for(what: &str, secs: u64, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(secs), "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

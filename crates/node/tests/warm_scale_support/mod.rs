//! What the two warm-copy scale evidence tests share: raw requests with a
//! long timeout, processor time by thread, and the joiner as its own process.

// ARITH: sums and means of small test numbers; the clippy.toml method bans
// are for shipping code, and each test file uses some of these.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods, dead_code)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::suite_rig::Cluster;

pub fn knob(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// One request on its own connection with a long read timeout; the status.
pub fn send(addr: SocketAddr, method: &str, target: &str, body: &str) -> (u16, String) {
    let Ok(mut s) = TcpStream::connect(addr) else {
        return (0, String::new());
    };
    s.set_read_timeout(Some(Duration::from_secs(900)))
        .expect("timeout");
    // No delay and one write: a head and a body in two small writes meet
    // Nagle's algorithm and the peer's delayed acknowledgement, which adds
    // about 40 ms to a request now and then and was read as a node's median
    // latency stepping from 10 to 50 ms.
    let _ = s.set_nodelay(true);
    let head = format!(
        "{method} {target} HTTP/1.1\r\nhost: test\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    let mut request = head.into_bytes();
    request.extend_from_slice(body.as_bytes());
    let _ = s.write_all(&request);
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    (status, text)
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("binds")
        .local_addr()
        .expect("addr")
        .port()
}

pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let at = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[at]
}

pub fn used(dir: &std::path::Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let mut total = 0_u64;
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        total += if p.is_dir() {
            used(&p)
        } else {
            p.metadata().map_or(0, |m| m.blocks() * 512)
        };
    }
    total
}

/// Processor seconds (user plus system) from a `/proc/.../stat` line.
pub fn stat_seconds(stat: &str) -> f64 {
    let after = stat.rsplit_once(')').map_or("", |(_, r)| r);
    let f: Vec<&str> = after.split_whitespace().collect();
    let ticks = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    (ticks(11) + ticks(12)) / 100.0
}

/// This process's processor seconds by thread name.
pub fn threads() -> BTreeMap<String, f64> {
    let mut by = BTreeMap::new();
    for t in std::fs::read_dir("/proc/self/task")
        .into_iter()
        .flatten()
        .flatten()
    {
        let stat = std::fs::read_to_string(t.path().join("stat")).unwrap_or_default();
        let name = stat
            .split_once('(')
            .and_then(|(_, r)| r.rsplit_once(')'))
            .map_or("?", |(n, _)| n)
            .trim_end_matches(|c: char| c.is_ascii_digit() || c == '-' || c == '_')
            .to_owned();
        *by.entry(name).or_insert(0.0) += stat_seconds(&stat);
    }
    by
}

pub fn process_seconds(pid: u32) -> f64 {
    stat_seconds(&std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default())
}

pub struct Joiner {
    pub child: Child,
    pub http: SocketAddr,
}

impl Drop for Joiner {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn spawn_joiner(
    cluster: &Cluster,
    tier: &std::path::Path,
    warm: bool,
    id: &str,
    extra: &[(String, String)],
) -> Joiner {
    let (endpoint, bucket, region, access, secret) = cluster.store();
    let http: SocketAddr = format!("127.0.0.1:{}", free_port()).parse().expect("addr");
    let transport = format!("127.0.0.1:{}", free_port());
    // `WARM_JOINER_CPUS` pins the joiner to those cores (`taskset -c`), so a
    // measurement can keep it off the cores the peer and the store run on.
    let mut command = match std::env::var("WARM_JOINER_CPUS") {
        Ok(cpus) if !cpus.is_empty() => {
            let mut c = Command::new("taskset");
            c.arg("-c").arg(cpus).arg(env!("CARGO_BIN_EXE_oss-node"));
            c
        }
        _ => Command::new(env!("CARGO_BIN_EXE_oss-node")),
    };
    command
        .env("OSS_NODE_ID", id)
        .env("OSS_STORE", "s3")
        .env("OSS_S3_ENDPOINT", endpoint)
        .env("OSS_S3_BUCKET", bucket)
        .env("OSS_S3_REGION", region)
        .env("OSS_S3_ACCESS_KEY", access)
        .env("OSS_S3_SECRET_KEY", secret)
        .env("OSS_HTTP_ADDR", http.to_string())
        .env("OSS_TRANSPORT_ADDR", transport)
        .env("OSS_LEASE_TTL_MS", "3000")
        .env("OSS_MEMORY_BYTES", "8589934592")
        .env("OSS_PAGE_CACHE_DIR", tier)
        .env("OSS_PAGE_CACHE_BYTES", "1073741824")
        .env("OSS_WARM_ON_JOIN", if warm { "true" } else { "false" })
        .env("OSS_WARM_MAX_WAIT_SECONDS", "600")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in extra {
        command.env(key, value);
    }
    let child = command.spawn().expect("the node binary starts");
    Joiner { child, http }
}

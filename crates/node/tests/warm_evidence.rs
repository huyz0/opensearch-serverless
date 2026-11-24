//! docs/specs/reader-warm-copy AC-8: what warming a joining reader costs
//! its peer and saves the joiner, on the emulator with request latency.
//! Ignored: it prints numbers for `VERIFIED.md` and takes minutes.
//!
//! `cargo test -p oss-node --all-features --test warm_evidence -- --ignored --nocapture`
//! Knobs: `WARM_EVIDENCE_DOCS` (default 20000), `WARM_EVIDENCE_SHARDS`
//! (default 1), `WARM_EVIDENCE_RUNS` (default 5).

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: sums and means of small test numbers.
#![allow(clippy::arithmetic_side_effects)]

mod suite_rig;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use oss_blob::S3Op;
use suite_rig::{Cluster, wait_for};

fn knob(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Processor time this process has used, in seconds (all nodes share it).
fn cpu_seconds() -> f64 {
    use std::io::Read;
    let mut buf = [0u8; 1024];
    let n = std::fs::File::open("/proc/self/stat")
        .and_then(|mut f| f.read(&mut buf))
        .unwrap_or(0);
    let stat = String::from_utf8_lossy(&buf[..n]).into_owned();
    let after = stat.rsplit_once(')').map_or("", |(_, r)| r);
    let f: Vec<&str> = after.split_whitespace().collect();
    let ticks = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    (ticks(11) + ticks(12)) / 100.0
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let at = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[at]
}

fn dir_bytes(dir: &std::path::Path) -> u64 {
    let mut total = 0;
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        total += if p.is_dir() {
            dir_bytes(&p)
        } else {
            p.metadata().map_or(0, |m| m.len())
        };
    }
    total
}

struct Run {
    peer_base_p99: f64,
    peer_during_p99: f64,
    peer_during_n: usize,
    cpu_during: f64,
    gets_up: u64,
    ready_ms: f64,
    first_search_ms: f64,
    gets_first: u64,
    joiner_bytes: u64,
    held_at_ready: u64,
}

fn run(warm: bool, docs: usize, shards: usize) -> Run {
    let root = std::env::temp_dir().join(format!("oss-warm-ev-{}-{warm}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut cluster = Cluster::new(3_000, 1, false).expect("emulator");
    cluster.set("OSS_PAGE_CACHE_BYTES", "1073741824");
    cluster.set("OSS_WARM_MAX_WAIT_SECONDS", "120");
    cluster.set("OSS_STORE_USAGE", "true");
    cluster.set("OSS_PAGE_CACHE_DIR", &root.join("n1").display().to_string());
    // A peer serves only with the switch on, so n1 has it on in both arms.
    cluster.set("OSS_WARM_ON_JOIN", "true");
    cluster.add("n1");
    let body = format!("{{\"settings\":{{\"index\":{{\"number_of_shards\":{shards}}}}}}}");
    assert_eq!(cluster.call(0, "PUT", "/logs", Some(&body)).status, 200);
    for chunk in (0..docs).collect::<Vec<_>>().chunks(2000) {
        let mut b = String::new();
        for i in chunk {
            b.push_str(&format!("{{\"index\":{{\"_id\":\"{i}\"}}}}\n"));
            b.push_str(&format!(
                "{{\"n\":{i},\"msg\":\"line {i} of the log with some words w{} x{} y{}\"}}\n",
                i % 97,
                i % 31,
                i % 7
            ));
        }
        let r = cluster.call(0, "POST", "/logs/_bulk?refresh=true", Some(&b));
        assert_eq!(r.status, 200, "{}", r.text());
    }
    let query = |addr: std::net::SocketAddr, i: usize| {
        let q = format!(
            "{{\"query\":{{\"match\":{{\"msg\":\"w{}\"}}}},\"size\":10}}",
            i % 97
        );
        let t = Instant::now();
        let r = suite_rig::request(addr, "POST", "/logs/_search", Some(q.as_bytes()));
        assert_eq!(r.status, 200, "{}", r.text());
        t.elapsed().as_secs_f64() * 1000.0
    };
    let peer = cluster.node(0).addr();
    for i in 0..300 {
        query(peer, i);
    }
    let mut base: Vec<f64> = (0..200).map(|i| query(peer, i)).collect();
    base.sort_by(f64::total_cmp);

    println!(
        "AC-8 peer tier bytes before join: {}",
        dir_bytes(&root.join("n1"))
    );
    let stop = AtomicBool::new(false);
    cluster.set("OSS_PAGE_CACHE_DIR", &root.join("n2").display().to_string());
    cluster.set("OSS_WARM_ON_JOIN", if warm { "true" } else { "false" });
    let gets0 = cluster.sent(S3Op::Get) + cluster.sent(S3Op::GetRange);
    let cpu0 = cpu_seconds();
    let mut during = Vec::new();
    let mut ready_ms = 0.0;
    std::thread::scope(|s| {
        let load = s.spawn(|| {
            let mut v = Vec::new();
            let mut i = 0;
            while !stop.load(Ordering::Acquire) {
                v.push(query(peer, i));
                i += 1;
            }
            v
        });
        let t = Instant::now();
        let slot = cluster.add("n2");
        wait_for("n2 ready", 150, || {
            cluster.call(slot, "GET", "/_serverless/ready", None).status == 200
        });
        ready_ms = t.elapsed().as_secs_f64() * 1000.0;
        stop.store(true, Ordering::Release);
        during = load.join().expect("load");
    });
    let cpu_during = cpu_seconds() - cpu0;
    let held_at_ready = dir_bytes(&root.join("n2"));
    let gets_up = cluster.sent(S3Op::Get) + cluster.sent(S3Op::GetRange) - gets0;
    during.sort_by(f64::total_cmp);
    let g1 = cluster.sent(S3Op::Get) + cluster.sent(S3Op::GetRange);
    let joiner = cluster.node(1).addr();
    let first_search_ms = query(joiner, 0);
    let gets_first = cluster.sent(S3Op::Get) + cluster.sent(S3Op::GetRange) - g1;
    let joiner_bytes = dir_bytes(&root.join("n2"));
    drop(cluster);
    let _ = std::fs::remove_dir_all(&root);
    Run {
        peer_base_p99: percentile(&base, 0.99),
        peer_during_p99: percentile(&during, 0.99),
        peer_during_n: during.len(),
        cpu_during,
        gets_up,
        ready_ms,
        first_search_ms,
        gets_first,
        joiner_bytes,
        held_at_ready,
    }
}

#[test]
#[ignore = "evidence: prints numbers, takes minutes"]
fn what_warming_costs_the_peer_and_saves_the_joiner() {
    let docs = knob("WARM_EVIDENCE_DOCS", 20_000);
    let shards = knob("WARM_EVIDENCE_SHARDS", 1);
    let runs = knob("WARM_EVIDENCE_RUNS", 5);
    println!(
        "AC-8 docs={docs} shards={shards} runs={runs} (emulator, Java latency profile; provisional)"
    );
    for warm in [false, true] {
        for n in 0..runs {
            let r = run(warm, docs, shards);
            println!(
                "AC-8 warm={warm} run={n} peer_p99_base={:.1}ms peer_p99_during={:.1}ms (n={}) cpu_s={:.2} gets_up={} ready={:.0}ms first_search={:.1}ms gets_first={} joiner_bytes={} held_at_ready={}",
                r.peer_base_p99,
                r.peer_during_p99,
                r.peer_during_n,
                r.cpu_during,
                r.gets_up,
                r.ready_ms,
                r.first_search_ms,
                r.gets_first,
                r.joiner_bytes,
                r.held_at_ready
            );
        }
    }
    let _ = Duration::ZERO;
}

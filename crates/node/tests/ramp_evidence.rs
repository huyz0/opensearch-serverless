//! docs/specs/reader-slow-start AC-9: what a joining reader's slow start
//! costs and saves, on the emulator with request latency. Ignored: it prints
//! numbers for `VERIFIED.md` and takes minutes.
//!
//! `cargo test -p oss-node --all-features --release --test ramp_evidence -- --ignored --nocapture`
//! Knobs: `RAMP_EVIDENCE_DOCS` (default 24000), `RAMP_EVIDENCE_SHARDS` (24),
//! `RAMP_EVIDENCE_PEERS` (3), `RAMP_EVIDENCE_SECS` (90), `RAMP_EVIDENCE_MAX`
//! (the ceiling, seconds; 60), `RAMP_EVIDENCE_MIN` (the floor; 20),
//! `RAMP_EVIDENCE_RUNS` (3).
//!
//! The nodes share one process, so the ramp setting and the count of searches
//! are the process's, not each node's: the joiner's controller sees every
//! node's forwarded searches against its own store reads, which makes it
//! raise its ramp sooner than a node alone in its process would.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: sums and means of small test numbers.
#![allow(clippy::arithmetic_side_effects)]

mod suite_rig;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use oss_blob::{BlobStore, S3Op};
use suite_rig::{Cluster, wait_for};

fn knob(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
}

/// The joiner's lease: when it said `search`, and its own ramp.
fn lease_ramp(cluster: &Cluster, id: &str) -> (Option<i64>, Option<i32>) {
    let (endpoint, bucket, region, access, secret) = cluster.store();
    let store = oss_blob::S3Store::new(oss_blob::S3Config::new(
        &endpoint,
        bucket,
        region,
        oss_blob::Credentials::new(access, secret),
    ))
    .expect("a store");
    let key = oss_format::lease_key("", id).expect("key");
    let mut buf = vec![0u8; 1 << 16];
    match store.read_register(&key, &mut buf) {
        Ok(Some(read)) => oss_format::NodeLease::parse(&buf[..read.len])
            .map(|l| (l.search_since, l.ramp_permille()))
            .unwrap_or((None, None)),
        _ => (None, None),
    }
}

struct Window {
    p50: f64,
    p99: f64,
    n: usize,
    gets: u64,
    ramp: Option<i32>,
}

fn run(on: bool, docs: usize, shards: usize, peers: usize, secs: usize, window: usize) -> Vec<Window> {
    let mut cluster = Cluster::new(3_000, 1, false).expect("emulator");
    cluster.set("OSS_STORE_USAGE", "true");
    for p in 0..peers {
        cluster.add(&format!("p{p}"));
    }
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
        // Distinct bodies, so neither a request cache nor one warm term answers them.
        let lo = (i * 37) % 20_000;
        let q = format!(
            "{{\"query\":{{\"bool\":{{\"must\":[{{\"match\":{{\"msg\":\"w{}\"}}}}],\"filter\":[{{\"range\":{{\"n\":{{\"gte\":{lo},\"lt\":{}}}}}}}]}}}},\"size\":10}}",
            i % 97,
            lo + 3_000
        );
        let t = Instant::now();
        let r = suite_rig::request(addr, "POST", "/logs/_search", Some(q.as_bytes()));
        assert_eq!(r.status, 200, "{}", r.text());
        t.elapsed().as_secs_f64() * 1000.0
    };
    // The entry node holds no role: it ranks readers for every shard and forwards.
    cluster.set("OSS_ROLES", "none");
    let slot = cluster.add("entry");
    cluster.set("OSS_ROLES", "ingest,search");
    let entry = cluster.node(slot).addr();
    wait_for("the entry sees the peers", 30, || cluster.nodes_seen(slot) as usize > peers);
    // The peers are warm: a few hundred searches over every shard.
    for i in 0..400 {
        query(entry, i);
    }
    if on {
        cluster.set("OSS_READER_RAMP_MAX_SECS", &knob("RAMP_EVIDENCE_MAX", 60).to_string());
        cluster.set("OSS_READER_RAMP_MIN_SECS", &knob("RAMP_EVIDENCE_MIN", 20).to_string());
    }
    let stop = AtomicBool::new(false);
    let gets0 = cluster.sent(S3Op::Get) + cluster.sent(S3Op::GetRange);
    let started = Instant::now();
    let mut samples: Vec<(f64, f64)> = Vec::new();
    let mut gets_at: Vec<u64> = Vec::new();
    let mut ramps: Vec<Option<i32>> = Vec::new();
    std::thread::scope(|s| {
        let load = s.spawn(|| {
            let mut v = Vec::new();
            let mut i = 0;
            while !stop.load(Ordering::Acquire) {
                let t = started.elapsed().as_secs_f64();
                v.push((t, query(entry, i)));
                i += 1;
            }
            v
        });
        let slot = cluster.add("joiner");
        wait_for("the joiner is ready", 60, || {
            cluster.call(slot, "GET", "/_serverless/ready", None).status == 200
        });
        for w in 1..=secs / window {
            let until = Duration::from_secs((w * window) as u64);
            while started.elapsed() < until {
                std::thread::sleep(Duration::from_millis(200));
            }
            gets_at.push(cluster.sent(S3Op::Get) + cluster.sent(S3Op::GetRange) - gets0);
            ramps.push(lease_ramp(&cluster, "joiner").1);
        }
        stop.store(true, Ordering::Release);
        samples = load.join().expect("load");
    });
    drop(cluster);
    let mut out = Vec::new();
    let mut last = 0;
    for (w, (gets, ramp)) in gets_at.iter().zip(&ramps).enumerate() {
        let lo = (w * window) as f64;
        let hi = ((w + 1) * window) as f64;
        let mut v: Vec<f64> = samples
            .iter()
            .filter(|(t, _)| *t >= lo && *t < hi)
            .map(|(_, ms)| *ms)
            .collect();
        v.sort_by(f64::total_cmp);
        out.push(Window {
            p50: percentile(&v, 0.5),
            p99: percentile(&v, 0.99),
            n: v.len(),
            gets: gets - last,
            ramp: *ramp,
        });
        last = *gets;
    }
    out
}

#[test]
#[ignore = "evidence: prints numbers, takes minutes"]
fn what_the_slow_start_costs_and_saves_a_joiner() {
    let docs = knob("RAMP_EVIDENCE_DOCS", 24_000);
    let shards = knob("RAMP_EVIDENCE_SHARDS", 24);
    let peers = knob("RAMP_EVIDENCE_PEERS", 3);
    let secs = knob("RAMP_EVIDENCE_SECS", 90);
    let runs = knob("RAMP_EVIDENCE_RUNS", 3);
    let window = 10;
    println!(
        "AC-9 docs={docs} shards={shards} peers={peers} secs={secs} runs={runs} (emulator, Java latency profile; provisional)"
    );
    for on in [false, true] {
        for n in 0..runs {
            let w = run(on, docs, shards, peers, secs, window);
            for (i, x) in w.iter().enumerate() {
                println!(
                    "AC-9 ramp={on} run={n} t={:>3}-{:>3}s n={:>5} p50={:>6.1}ms p99={:>7.1}ms gets={:>5} joiner_ramp={:?}",
                    i * window,
                    (i + 1) * window,
                    x.n,
                    x.p50,
                    x.p99,
                    x.gets,
                    x.ramp
                );
            }
        }
    }
}

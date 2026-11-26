//! docs/specs/reader-warm-copy AC-8: what warming a joiner costs a peer that
//! holds many shards, with the joiner a separate process so its processor
//! time is its own. The peer, the emulator with the Java latency profile and
//! the paced load share this process, so the peer's processor time is split by
//! thread name. Ignored: it prints numbers for `VERIFIED.md` and takes minutes.
//!
//! `cargo test -p oss-node --all-features --test warm_scale_evidence -- --ignored --nocapture`
//! Knobs: `WARM_SCALE_SHARDS` (default 1000), `WARM_SCALE_DOCS` (default 20
//! a shard), `WARM_SCALE_RUNS` (default 3), `WARM_SCALE_WINDOW` (seconds of
//! paced load, default 40), `WARM_SCALE_GAP_MS` (between queries, default 100).

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: sums and means of small test numbers; the clippy.toml method bans
// are for shipping code.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

mod suite_rig;
mod warm_scale_support;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use suite_rig::Cluster;
use warm_scale_support::{knob, percentile, process_seconds, send, spawn_joiner, threads, used};

struct Run {
    base_p99: f64,
    during_p99: f64,
    during_max: f64,
    during_n: usize,
    ready_s: f64,
    first_search_ms: f64,
    joiner_cpu_s: f64,
    joiner_bytes: u64,
    peer_cpu_by_thread: BTreeMap<String, f64>,
    peer_cpu_total: f64,
    span_s: f64,
}

fn run(warm: bool, shards: usize, docs: usize, window: Duration, gap: Duration) -> Run {
    let root = std::env::temp_dir().join(format!("oss-warm-scale-{}-{warm}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut cluster = Cluster::new(3_000, 1, false).expect("emulator");
    cluster.set("OSS_PAGE_CACHE_BYTES", "1073741824");
    cluster.set("OSS_PAGE_CACHE_DIR", &root.join("n1").display().to_string());
    cluster.set("OSS_WARM_ON_JOIN", "true");
    cluster.add("n1");
    let peer = cluster.node(0).addr();
    let created = Instant::now();
    let body = format!("{{\"settings\":{{\"index\":{{\"number_of_shards\":{shards}}}}}}}");
    let (status, text) = send(peer, "PUT", "/logs", &body);
    assert_eq!(status, 200, "{text}");
    println!(
        "AC-8 scale: index of {shards} shards created in {:.1}s",
        created.elapsed().as_secs_f64()
    );
    let loaded = Instant::now();
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
        let (status, text) = send(peer, "POST", "/logs/_bulk?refresh=true", &b);
        assert_eq!(status, 200, "{text}");
    }
    println!(
        "AC-8 scale: {docs} documents loaded in {:.1}s",
        loaded.elapsed().as_secs_f64()
    );
    let query = |addr: SocketAddr, i: usize| {
        let q = format!(
            "{{\"query\":{{\"match\":{{\"msg\":\"w{}\"}}}},\"size\":10}}",
            i % 97
        );
        let t = Instant::now();
        let (status, text) = send(addr, "POST", "/logs/_search", &q);
        assert_eq!(status, 200, "{text}");
        t.elapsed().as_secs_f64() * 1000.0
    };
    for i in 0..100 {
        query(peer, i);
    }
    let mut base: Vec<f64> = (0..100).map(|i| query(peer, i)).collect();
    base.sort_by(f64::total_cmp);
    let tier_a = used(&root.join("n1"));
    println!("AC-8 scale: peer tier holds {tier_a} bytes");

    let cpu0 = threads();
    let started = Instant::now();
    let joiner = spawn_joiner(&cluster, &root.join("n2"), warm, "n2", &[]);
    let pid = joiner.child.id();
    let mut during = Vec::new();
    let mut ready_s = 0.0;
    let mut joiner_cpu_at_ready = 0.0;
    let mut i = 0;
    // At least the window; past it only while the joiner is not yet ready.
    while started.elapsed() < window
        || (ready_s == 0.0 && started.elapsed() < Duration::from_secs(900))
    {
        let tick = Instant::now();
        during.push(query(peer, i));
        i += 1;
        if ready_s == 0.0 {
            let (status, _) = send(joiner.http, "GET", "/_serverless/ready", "");
            if status == 200 {
                ready_s = started.elapsed().as_secs_f64();
                joiner_cpu_at_ready = process_seconds(pid);
            }
        }
        if let Some(rest) = gap.checked_sub(tick.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    assert!(ready_s > 0.0, "the joiner was not ready inside the window");
    let _ = joiner_cpu_at_ready;
    let span_s = started.elapsed().as_secs_f64();
    let cpu1 = threads();
    let first_search_ms = query(joiner.http, 0);
    let joiner_cpu_s = process_seconds(pid);
    let joiner_bytes = used(&root.join("n2"));
    let mut peer_cpu_by_thread = BTreeMap::new();
    for (name, now) in &cpu1 {
        let before = cpu0.get(name).copied().unwrap_or(0.0);
        if now - before >= 0.05 {
            peer_cpu_by_thread.insert(name.clone(), now - before);
        }
    }
    let peer_cpu_total = cpu1.values().sum::<f64>() - cpu0.values().sum::<f64>();
    during.sort_by(f64::total_cmp);
    drop(joiner);
    drop(cluster);
    let _ = std::fs::remove_dir_all(&root);
    Run {
        base_p99: percentile(&base, 0.99),
        during_p99: percentile(&during, 0.99),
        during_max: during.last().copied().unwrap_or(0.0),
        during_n: during.len(),
        ready_s,
        first_search_ms,
        joiner_cpu_s,
        joiner_bytes,
        peer_cpu_by_thread,
        peer_cpu_total,
        span_s,
    }
}

#[test]
#[ignore = "evidence: prints numbers, takes minutes"]
fn what_warming_costs_a_peer_holding_many_shards() {
    let shards = knob("WARM_SCALE_SHARDS", 1000);
    let docs = knob("WARM_SCALE_DOCS", shards * 20);
    let runs = knob("WARM_SCALE_RUNS", 3);
    let window = Duration::from_secs(knob("WARM_SCALE_WINDOW", 40) as u64);
    let gap = Duration::from_millis(knob("WARM_SCALE_GAP_MS", 100) as u64);
    println!(
        "AC-8 scale shards={shards} docs={docs} runs={runs} window={window:?} gap={gap:?} (emulator, Java latency profile; provisional)"
    );
    // `WARM_SCALE_ARMS=on` or `off` runs one arm.
    let arms = std::env::var("WARM_SCALE_ARMS").unwrap_or_default();
    for warm in [false, true] {
        if (arms == "on" && !warm) || (arms == "off" && warm) {
            continue;
        }
        for n in 0..runs {
            let r = run(warm, shards, docs, window, gap);
            println!(
                "AC-8 scale warm={warm} run={n} peer_p99_base={:.1}ms peer_p99_during={:.1}ms peer_max_during={:.1}ms (n={}) ready={:.1}s first_search={:.1}ms joiner_cpu_s={:.2} joiner_bytes={} peer_cpu_total_s={:.2} over {:.0}s",
                r.base_p99,
                r.during_p99,
                r.during_max,
                r.during_n,
                r.ready_s,
                r.first_search_ms,
                r.joiner_cpu_s,
                r.joiner_bytes,
                r.peer_cpu_total,
                r.span_s
            );
            for (name, secs) in &r.peer_cpu_by_thread {
                println!("AC-8 scale   warm={warm} run={n} peer thread {name}: {secs:.2}s");
            }
        }
    }
}

//! docs/specs/reader-warm-copy AC-8: the joiner's open concurrency and copy
//! rate against one peer holding many shards, loaded once. Ignored: it prints
//! numbers for `VERIFIED.md` and takes most of an hour.
//!
//! `WARM_SWEEP="off;open8:OSS_WARM_OPEN_PARALLEL=8" cargo test -p oss-node --all-features --test warm_sweep_evidence -- --ignored --nocapture`
//! Knobs: `WARM_SCALE_SHARDS` (default 1000), `WARM_SCALE_DOCS` (default 4
//! a shard), `WARM_SCALE_RUNS` (rounds, default 2), `WARM_SCALE_WINDOW`
//! (seconds, default 60), `WARM_SCALE_GAP_MS` (default 100).

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: sums and means of small test numbers; the clippy.toml method bans
// are for shipping code.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

mod suite_rig;
mod warm_scale_support;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use suite_rig::Cluster;
use warm_scale_support::{knob, percentile, process_seconds, send, spawn_joiner, threads};

/// One joiner run's numbers against an already loaded peer.
struct Sweep {
    base_p50: f64,
    base_p99: f64,
    p50: f64,
    p95: f64,
    p99: f64,
    ready_s: f64,
    first_search_ms: f64,
    joiner_cpu_s: f64,
    peer_cores: f64,
    n: usize,
}

fn sweep_run(
    cluster: &Cluster,
    root: &std::path::Path,
    warm: bool,
    id: &str,
    extra: &[(String, String)],
    min_window: Duration,
    gap: Duration,
) -> Sweep {
    let peer = cluster.node(0).addr();
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
    let mut base: Vec<f64> = (0..100).map(|i| query(peer, i)).collect();
    base.sort_by(f64::total_cmp);
    let tier = root.join(id);
    let cpu0 = threads();
    let started = Instant::now();
    let joiner = spawn_joiner(cluster, &tier, warm, id, extra);
    let pid = joiner.child.id();
    let mut during = Vec::new();
    let mut ready_s = 0.0;
    let mut i = 0;
    // Until the joiner is ready and the window is over.
    while started.elapsed() < min_window
        || (ready_s == 0.0 && started.elapsed() < Duration::from_secs(600))
    {
        let tick = Instant::now();
        during.push(query(peer, i));
        i += 1;
        if ready_s == 0.0 && send(joiner.http, "GET", "/_serverless/ready", "").0 == 200 {
            ready_s = started.elapsed().as_secs_f64();
        }
        if let Some(rest) = gap.checked_sub(tick.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    assert!(ready_s > 0.0, "the joiner was not ready");
    let span = started.elapsed().as_secs_f64();
    let cpu1 = threads();
    let first_search_ms = query(joiner.http, 0);
    let joiner_cpu_s = process_seconds(pid);
    during.sort_by(f64::total_cmp);
    drop(joiner);
    let _ = std::fs::remove_dir_all(&tier);
    Sweep {
        base_p50: percentile(&base, 0.5),
        base_p99: percentile(&base, 0.99),
        p50: percentile(&during, 0.5),
        p95: percentile(&during, 0.95),
        p99: percentile(&during, 0.99),
        ready_s,
        first_search_ms,
        joiner_cpu_s,
        peer_cores: (cpu1.values().sum::<f64>() - cpu0.values().sum::<f64>()) / span,
        n: during.len(),
    }
}

/// Configurations come as `label:KEY=value,KEY=value` separated by `;`; the
/// label `off` runs the joiner with the copy off.
type Config = (String, bool, Vec<(String, String)>);

fn configs() -> Vec<Config> {
    let text = std::env::var("WARM_SWEEP").unwrap_or_else(|_| {
        "off;open2:OSS_WARM_OPEN_PARALLEL=2;open4:OSS_WARM_OPEN_PARALLEL=4;open8:OSS_WARM_OPEN_PARALLEL=8;open16:OSS_WARM_OPEN_PARALLEL=16".to_owned()
    });
    text.split(';')
        .map(|c| {
            let (label, rest) = c.split_once(':').unwrap_or((c, ""));
            let pairs = rest
                .split(',')
                .filter_map(|kv| kv.split_once('='))
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect();
            (label.to_owned(), label != "off", pairs)
        })
        .collect()
}

#[test]
#[ignore = "evidence: prints numbers, takes an hour"]
fn the_joiner_settings_against_one_peer_holding_many_shards() {
    let shards = knob("WARM_SCALE_SHARDS", 1000);
    let docs = knob("WARM_SCALE_DOCS", shards * 4);
    let rounds = knob("WARM_SCALE_RUNS", 2);
    let window = Duration::from_secs(knob("WARM_SCALE_WINDOW", 60) as u64);
    let gap = Duration::from_millis(knob("WARM_SCALE_GAP_MS", 100) as u64);
    let root = std::env::temp_dir().join(format!("oss-warm-sweep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut cluster = Cluster::new(3_000, 1, false).expect("emulator");
    cluster.set("OSS_PAGE_CACHE_BYTES", "1073741824");
    cluster.set("OSS_PAGE_CACHE_DIR", &root.join("n1").display().to_string());
    cluster.set("OSS_WARM_ON_JOIN", "true");
    cluster.add("n1");
    let peer = cluster.node(0).addr();
    let body = format!("{{\"settings\":{{\"index\":{{\"number_of_shards\":{shards}}}}}}}");
    assert_eq!(send(peer, "PUT", "/logs", &body).0, 200);
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
        assert_eq!(send(peer, "POST", "/logs/_bulk?refresh=true", &b).0, 200);
    }
    for i in 0..200 {
        let q = format!(
            "{{\"query\":{{\"match\":{{\"msg\":\"w{}\"}}}},\"size\":10}}",
            i % 97
        );
        assert_eq!(send(peer, "POST", "/logs/_search", &q).0, 200);
    }
    println!(
        "SWEEP shards={shards} docs={docs} rounds={rounds} window={window:?} gap={gap:?} (emulator, Java latency profile; provisional)"
    );
    let all = configs();
    let mut k = 0;
    for round in 0..rounds {
        for (label, warm, extra) in &all {
            k += 1;
            let id = format!("j{k}");
            let r = sweep_run(&cluster, &root, *warm, &id, extra, window, gap);
            println!(
                "SWEEP {label} round={round} ready={:.1}s first_search={:.0}ms peer_p50={:.0}ms (base {:.0}) peer_p95={:.0}ms peer_p99={:.0}ms (base {:.0}) n={} peer_cores={:.2} joiner_cpu_s={:.1}",
                r.ready_s,
                r.first_search_ms,
                r.p50,
                r.base_p50,
                r.p95,
                r.p99,
                r.base_p99,
                r.n,
                r.peer_cores,
                r.joiner_cpu_s
            );
            // The killed joiner's lease lapses before the next one starts.
            std::thread::sleep(Duration::from_secs(12));
        }
    }
    drop(cluster);
    let _ = std::fs::remove_dir_all(&root);
}

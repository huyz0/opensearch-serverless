//! docs/specs/reader-warm-copy AC-8: the peer's search latency while a joiner
//! warms, measured as alternating arms against one loaded peer. The peer is
//! loaded once (loading dominates a run on RustFS); each round then starts a
//! fresh joiner process, copy off or on in turn, and compares the peer's
//! searches over the window after the join with the searches just before it.
//! Ignored: it prints numbers for `VERIFIED.md` and takes minutes.
//!
//! `cargo test -p oss-node --all-features --test warm_interleaved_evidence -- --ignored --nocapture`
//! Knobs: `WARM_SCALE_SHARDS` (1000), `WARM_SCALE_DOCS` (20 a shard),
//! `WARM_ROUNDS` (arm pairs, 5), `WARM_SCALE_WINDOW` (seconds, 40),
//! `WARM_SCALE_GAP_MS` (100), `WARM_SCALE_RUSTFS` (with `RUSTFS_BIN`),
//! `WARM_JOINER_CPUS` (cores for the joiner, `taskset -c` syntax), and
//! `WARM_JOINER_ENV` (`K=V,K=V` for the joiner only).

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
use warm_scale_support::{knob, percentile, process_seconds, send, spawn_joiner, threads};

fn query(addr: SocketAddr, i: usize) -> f64 {
    let q = format!(
        "{{\"query\":{{\"match\":{{\"msg\":\"w{}\"}}}},\"size\":10}}",
        i % 97
    );
    let t = Instant::now();
    let (status, text) = send(addr, "POST", "/logs/_search", &q);
    assert_eq!(status, 200, "{text}");
    t.elapsed().as_secs_f64() * 1000.0
}

/// Median, p95, p99 and max of `ms`, which this sorts.
fn stats(ms: &mut [f64]) -> (f64, f64, f64, f64) {
    ms.sort_by(f64::total_cmp);
    (
        percentile(ms, 0.5),
        percentile(ms, 0.95),
        percentile(ms, 0.99),
        ms.last().copied().unwrap_or(0.0),
    )
}

#[test]
#[ignore = "evidence: prints numbers, takes minutes"]
fn the_peers_search_latency_while_a_joiner_warms_alternating_arms() {
    let shards = knob("WARM_SCALE_SHARDS", 1000);
    let docs = knob("WARM_SCALE_DOCS", shards * 20);
    let rounds = knob("WARM_ROUNDS", 5);
    let window = Duration::from_secs(knob("WARM_SCALE_WINDOW", 40) as u64);
    let gap = Duration::from_millis(knob("WARM_SCALE_GAP_MS", 100) as u64);
    let rustfs = std::env::var_os("WARM_SCALE_RUSTFS").is_some();
    let extra: Vec<(String, String)> = std::env::var("WARM_JOINER_ENV")
        .unwrap_or_default()
        .split(',')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    let root = std::env::temp_dir().join(format!("oss-warm-inter-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut cluster = Cluster::new(3_000, 1, rustfs).expect("the emulator, or RUSTFS_BIN");
    cluster.set("OSS_PAGE_CACHE_BYTES", "1073741824");
    cluster.set("OSS_PAGE_CACHE_DIR", &root.join("n1").display().to_string());
    cluster.set("OSS_WARM_ON_JOIN", "true");
    cluster.add("n1");
    let peer = cluster.node(0).addr();
    let body = format!("{{\"settings\":{{\"index\":{{\"number_of_shards\":{shards}}}}}}}");
    let (status, text) = send(peer, "PUT", "/logs", &body);
    assert_eq!(status, 200, "{text}");
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
        "AC-8 inter: {docs} documents in {shards} shards loaded in {:.1}s (store: {}; joiner cpus {:?}; provisional)",
        loaded.elapsed().as_secs_f64(),
        if rustfs { "RustFS" } else { "emulator" },
        std::env::var("WARM_JOINER_CPUS").ok()
    );
    for i in 0..300 {
        query(peer, i);
    }
    let mut results: BTreeMap<bool, Vec<(f64, f64, f64, f64, f64, f64)>> = BTreeMap::new();
    let mut i = 0;
    for round in 0..rounds {
        // Alternate which arm goes first so a drift does not favour one.
        let order = if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        };
        for warm in order {
            std::thread::sleep(Duration::from_secs(5));
            let mut before: Vec<f64> = (0..150)
                .map(|_| {
                    i += 1;
                    let t = Instant::now();
                    let v = query(peer, i);
                    if let Some(rest) = gap.checked_sub(t.elapsed()) {
                        std::thread::sleep(rest);
                    }
                    v
                })
                .collect();
            let cpu0 = threads();
            let started = Instant::now();
            let tier = root.join(format!("n2-{round}-{warm}"));
            let joiner = spawn_joiner(&cluster, &tier, warm, "n2", &extra);
            let pid = joiner.child.id();
            let mut during = Vec::new();
            let mut ready = 0.0;
            while started.elapsed() < window
                || (ready == 0.0 && started.elapsed() < Duration::from_secs(900))
            {
                let t = Instant::now();
                i += 1;
                during.push(query(peer, i));
                if ready == 0.0 {
                    let (status, _) = send(joiner.http, "GET", "/_serverless/ready", "");
                    if status == 200 {
                        ready = started.elapsed().as_secs_f64();
                    }
                }
                if let Some(rest) = gap.checked_sub(t.elapsed()) {
                    std::thread::sleep(rest);
                }
            }
            let cpu1 = threads();
            let peer_cpu = cpu1.values().sum::<f64>() - cpu0.values().sum::<f64>();
            let first = query(joiner.http, 0);
            let jcpu = process_seconds(pid);
            let (bm, _, bp99, _) = stats(&mut before);
            let (dm, d95, dp99, dmax) = stats(&mut during);
            println!(
                "AC-8 inter round={round} warm={warm} before: med={bm:.1} p99={bp99:.1} | during: med={dm:.1} p95={d95:.1} p99={dp99:.1} max={dmax:.1} (n={}) | ready={ready:.1}s first={first:.1}ms joiner_cpu={jcpu:.1}s peer_cpu={peer_cpu:.1}s",
                during.len()
            );
            results
                .entry(warm)
                .or_default()
                .push((bm, bp99, dm, dp99, dmax, d95));
            drop(joiner);
            let _ = std::fs::remove_dir_all(&tier);
        }
    }
    for (warm, rs) in &results {
        let mut dp99: Vec<f64> = rs.iter().map(|r| r.3).collect();
        let mut dm: Vec<f64> = rs.iter().map(|r| r.2).collect();
        let mut ratio: Vec<f64> = rs.iter().map(|r| r.3 / r.1).collect();
        dp99.sort_by(f64::total_cmp);
        dm.sort_by(f64::total_cmp);
        ratio.sort_by(f64::total_cmp);
        println!(
            "AC-8 inter SUMMARY warm={warm} rounds={}: during p99 median={:.1} min={:.1} max={:.1}; during median-latency median={:.1}; p99/before-p99 median={:.2} min={:.2} max={:.2}",
            rs.len(),
            percentile(&dp99, 0.5),
            dp99[0],
            dp99[dp99.len() - 1],
            percentile(&dm, 0.5),
            percentile(&ratio, 0.5),
            ratio[0],
            ratio[ratio.len() - 1]
        );
    }
    drop(cluster);
    let _ = std::fs::remove_dir_all(&root);
}

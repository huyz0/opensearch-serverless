//! docs/specs/reader-warm-copy AC-11: the warm-copy limits against one loaded
//! peer. The peer (its own `OSS_WARM_*` settings from `WARM_PEER_ENV`) is
//! loaded once with documents of `WARM_DOC_BYTES` each, then each round starts
//! a fresh joiner for every config in `WARM_CONFIGS` and for the copy-off arm,
//! and prints the peer's p99 over the copy (until the joiner is ready, at least
//! `WARM_SCALE_WINDOW` seconds in all), the joiner's time to ready and the
//! bytes its tier holds then. Ignored: it prints numbers for `VERIFIED.md`.
//!
//! `cargo test --release -p oss-node --all-features --test warm_limits_sweep_evidence -- --ignored --nocapture`
//! Knobs: `WARM_SCALE_SHARDS` (1000), `WARM_SCALE_DOCS` (4000), `WARM_DOC_BYTES`
//! (60000), `WARM_ROUNDS` (2), `WARM_SCALE_WINDOW` (30), `WARM_SCALE_GAP_MS`
//! (20), `WARM_SCALE_RUSTFS` (with the RustFS binary variable), `WARM_JOINER_CPUS`,
//! `WARM_PEER_ENV` (`K=V,K=V` for the peer), `WARM_CONFIGS`
//! (`label:K=V;K=V|label:K=V`, joiner settings per config),
//! `WARM_JOINER_TIER_ROOT` (a directory for the joiners' tiers).

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: sums and means of small test numbers; the clippy.toml method bans
// are for shipping code.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

mod suite_rig;
mod warm_scale_support;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use suite_rig::Cluster;
use warm_scale_support::{knob, percentile, send, spawn_joiner, used};

/// Environment variables as pairs.
type Settings = Vec<(String, String)>;

/// This process's user and system CPU time so far, in seconds: the peer runs
/// in it, with the load client.
fn cpu_seconds() -> f64 {
    cpu_seconds_of("self")
}

/// The same for the process `who` (`self` or a pid).
fn cpu_seconds_of(who: &str) -> f64 {
    let stat = std::fs::read_to_string(format!("/proc/{who}/stat")).unwrap_or_default();
    // The fields after the command's closing parenthesis; utime and stime
    // are the 12th and 13th of those, in clock ticks of 1/100 s.
    let rest = stat.rsplit_once(')').map_or("", |(_, r)| r);
    let f: Vec<f64> = rest
        .split_whitespace()
        .skip(11)
        .take(2)
        .filter_map(|v| v.parse().ok())
        .collect();
    f.iter().sum::<f64>() / 100.0
}

/// A `Vm...` figure of `/proc/<who>/status` in MiB (`VmHWM` is the peak
/// resident size, `VmRSS` the current one).
fn vm_mib(who: &str, field: &str) -> f64 {
    let status = std::fs::read_to_string(format!("/proc/{who}/status")).unwrap_or_default();
    status
        .lines()
        .find_map(|l| l.strip_prefix(field)?.trim_start_matches(':').split_whitespace().next()?.parse::<f64>().ok())
        .unwrap_or(0.0)
        / 1024.0
}

fn query(addr: SocketAddr, i: usize, size: usize) -> f64 {
    let q = format!(
        "{{\"query\":{{\"match\":{{\"msg\":\"w{}\"}}}},\"size\":{size}}}",
        i % 97
    );
    let t = Instant::now();
    let (status, text) = send(addr, "POST", "/logs/_search", &q);
    assert_eq!(status, 200, "{text}");
    t.elapsed().as_secs_f64() * 1000.0
}

fn pairs(spec: &str, outer: char, inner: char) -> Settings {
    spec.split(outer)
        .filter_map(|kv| kv.split_once(inner))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

#[test]
#[ignore = "evidence: prints numbers, takes tens of minutes"]
fn the_warm_copy_limits_against_one_peer() {
    let shards = knob("WARM_SCALE_SHARDS", 1000);
    let docs = knob("WARM_SCALE_DOCS", 4000);
    let doc_bytes = knob("WARM_DOC_BYTES", 60_000);
    let rounds = knob("WARM_ROUNDS", 2);
    let window = Duration::from_secs(knob("WARM_SCALE_WINDOW", 30) as u64);
    let gap = Duration::from_millis(knob("WARM_SCALE_GAP_MS", 20) as u64);
    let rustfs = std::env::var_os("WARM_SCALE_RUSTFS").is_some();
    let peer_env = pairs(
        &std::env::var("WARM_PEER_ENV").unwrap_or_default(),
        ',',
        '=',
    );
    let configs: Vec<(String, Settings)> = std::env::var("WARM_CONFIGS")
        .unwrap_or_default()
        .split('|')
        .filter_map(|c| c.split_once(':'))
        .map(|(label, kv)| (label.to_owned(), pairs(kv, ';', '=')))
        .collect();
    // `WARM_NO_SEARCH=1` sends the peer no searches, so what it spends is the
    // copy's alone.
    let no_search = std::env::var("WARM_NO_SEARCH").is_ok_and(|v| v == "1");
    let root = std::env::temp_dir().join(format!("oss-warm-sweep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut cluster = Cluster::new(3_000, 1, rustfs).expect("the emulator, or the RustFS binary");
    cluster.set("OSS_PAGE_CACHE_BYTES", "8589934592");
    cluster.set("OSS_PAGE_CACHE_DIR", &root.join("n1").display().to_string());
    cluster.set("OSS_WARM_ON_JOIN", "true");
    for (k, v) in &peer_env {
        cluster.set(k, v);
    }
    cluster.add("n1");
    let peer = cluster.node(0).addr();
    let body = format!("{{\"settings\":{{\"index\":{{\"number_of_shards\":{shards}}}}}}}");
    let (status, text) = send(peer, "PUT", "/logs", &body);
    assert_eq!(status, 200, "{text}");
    // Words from a vocabulary of a few thousand, so the stored bytes
    // compress by a modest factor and not to nothing.
    let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let loaded = Instant::now();
    for chunk in (0..docs).collect::<Vec<_>>().chunks(100) {
        let mut b = String::new();
        for i in chunk {
            b.push_str(&format!("{{\"index\":{{\"_id\":\"{i}\"}}}}\n"));
            let mut text = format!("line {i} w{} x{} ", i % 97, i % 31);
            while text.len() < doc_bytes {
                text.push_str(&format!("t{} ", next() % 6000));
            }
            b.push_str(&format!("{{\"n\":{i},\"msg\":\"{text}\"}}\n"));
        }
        let (status, text) = send(peer, "POST", "/logs/_bulk?refresh=true", &b);
        assert_eq!(status, 200, "{}", &text[..text.len().min(300)]);
    }
    println!(
        "AC-11 sweep: {docs} documents of {doc_bytes} bytes in {shards} shards loaded in {:.1}s (store: {}; peer env {peer_env:?}; provisional)",
        loaded.elapsed().as_secs_f64(),
        if rustfs { "RustFS" } else { "emulator" }
    );
    for i in 0..600 {
        query(peer, i, 40);
    }
    println!(
        "AC-11 sweep: peer tier holds {} bytes",
        used(&root.join("n1"))
    );
    let mut arms: Vec<(String, bool, Settings)> = vec![("copy-off".into(), false, Vec::new())];
    for (label, kv) in configs {
        arms.push((label, true, kv));
    }
    let mut i = 0;
    for round in 0..rounds {
        for (label, warm, extra) in &arms {
            std::thread::sleep(Duration::from_secs(5));
            // `WARM_JOINER_TIER_ROOT` puts the joiners' tiers elsewhere (for
            // example tmpfs), so what they write does not share the peer's
            // disk and its page cache.
            // A config's own `TIER_ROOT` wins over the variable.
            let tier = extra
                .iter()
                .find(|(k, _)| k == "TIER_ROOT")
                .map(|(_, v)| v.clone())
                .or_else(|| std::env::var("WARM_JOINER_TIER_ROOT").ok())
                .map_or_else(|| root.clone(), std::path::PathBuf::from)
                .join(format!(
                    "oss-warm-n2-{}-{round}-{label}",
                    std::process::id()
                ));
            let started = Instant::now();
            let cpu_before = cpu_seconds();
            let rss_before = vm_mib("self", "VmRSS");
            let hwm_before = vm_mib("self", "VmHWM");
            // `JOINERS=n` in a config starts n joiners together (each with its
            // own tier), and ready is when the last one is.
            let count = extra
                .iter()
                .find(|(k, _)| k == "JOINERS")
                .and_then(|(_, v)| v.parse::<usize>().ok())
                .unwrap_or(1);
            let env: Settings = extra
                .iter()
                .filter(|(k, _)| k != "JOINERS" && k != "TIER_ROOT")
                .cloned()
                .collect();
            let mut joiners = Vec::new();
            for n in 0..count {
                let id = format!("n{}", n + 2);
                joiners.push(spawn_joiner(&cluster, &tier.join(&id), *warm, &id, &env));
            }
            let joiner = &joiners[0];
            let mut ready_at = vec![0.0_f64; joiners.len()];
            let mut ms = Vec::new();
            let mut ready = 0.0;
            let mut held_at_ready = 0;
            while started.elapsed() < window
                || (ready == 0.0 && started.elapsed() < Duration::from_secs(900))
            {
                let t = Instant::now();
                i += 1;
                if no_search {
                    std::thread::sleep(Duration::from_millis(200));
                } else {
                    ms.push(query(peer, i, 10));
                }
                for (at, j) in ready_at.iter_mut().zip(&joiners) {
                    if *at == 0.0 && send(j.http, "GET", "/_serverless/ready", "").0 == 200 {
                        *at = started.elapsed().as_secs_f64();
                    }
                }
                if ready == 0.0 && ready_at.iter().all(|a| *a > 0.0) {
                    ready = ready_at.iter().copied().fold(0.0, f64::max);
                    held_at_ready = used(&tier);
                }
                if let Some(rest) = gap.checked_sub(t.elapsed()) {
                    std::thread::sleep(rest);
                }
            }
            ms.sort_by(f64::total_cmp);
            let cores = (cpu_seconds() - cpu_before) / started.elapsed().as_secs_f64();
            let peer_rss = vm_mib("self", "VmRSS") - rss_before;
            let peer_peak = vm_mib("self", "VmHWM") - hwm_before;
            let joiner_cpu: f64 = joiners
                .iter()
                .map(|j| cpu_seconds_of(&j.child.id().to_string()))
                .sum();
            let joiner_peak = joiners
                .iter()
                .map(|j| vm_mib(&j.child.id().to_string(), "VmHWM"))
                .fold(0.0, f64::max);
            let first = query(joiner.http, 0, 10);
            println!(
                "AC-11 sweep round={round} config={label}: peer during copy+window med={:.1} p99={:.1} max={:.1} (n={}) | ready={ready:.1}s first_search={first:.1}ms joiner_tier_at_ready={held_at_ready} peer_cores_busy={cores:.2} peer_rss_delta_mib={peer_rss:.0} peer_peak_delta_mib={peer_peak:.0} joiner_cpu_s={joiner_cpu:.1} joiner_peak_mib={joiner_peak:.0}",
                percentile(&ms, 0.5),
                percentile(&ms, 0.99),
                ms.last().copied().unwrap_or(0.0),
                ms.len()
            );
            drop(joiners);
            let _ = std::fs::remove_dir_all(&tier);
        }
    }
    drop(cluster);
    let _ = std::fs::remove_dir_all(&root);
}

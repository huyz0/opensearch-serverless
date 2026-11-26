//! docs/specs/reader-warm-copy AC-7 on booted nodes: a warming joiner answers
//! 503 until `OSS_WARM_READY_PERCENT` of the bytes its peer named are in its
//! disk tier, with the copy throttled so that point is seconds from the start.
//! Ignored: it prints the ready time and poll count for `VERIFIED.md`. The
//! bytes named and copied at the moment of ready are not exposed by the node;
//! they were read from a temporary stderr line in `finish_when_warm`.
//!
//! `cargo test -p oss-node --all-features --test warm_ready_evidence -- --ignored --nocapture`
//! Knobs: `WARM_READY_DOCS` (default 20000), `WARM_READY_RATE` (bytes a second,
//! default 524288).

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: sums and ratios of small test numbers.
#![allow(clippy::arithmetic_side_effects)]

mod suite_rig;

use std::time::{Duration, Instant};

use suite_rig::Cluster;

fn knob(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn dir_bytes(dir: &std::path::Path) -> u64 {
    let mut total = 0;
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        total += if p.is_dir() {
            dir_bytes(&p)
        } else {
            {
                use std::os::unix::fs::MetadataExt;
                // Blocks in use: the tier's files are sized up front.
                p.metadata().map_or(0, |m| m.blocks() * 512)
            }
        };
    }
    total
}

struct Run {
    percent: u64,
    peer_bytes: u64,
    not_ready_samples: usize,
    early_ready: bool,
    ready_ms: f64,
}

fn run(percent: u64, docs: usize, rate: usize) -> Run {
    let root = std::env::temp_dir().join(format!("oss-warm-rdy-{}-{percent}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut cluster = Cluster::new(3_000, 1, false).expect("emulator");
    cluster.set("OSS_PAGE_CACHE_BYTES", "1073741824");
    cluster.set("OSS_WARM_MAX_WAIT_SECONDS", "600");
    cluster.set("OSS_PAGE_CACHE_DIR", &root.join("n1").display().to_string());
    cluster.set("OSS_WARM_ON_JOIN", "true");
    cluster.set("OSS_STORE_USAGE", "true");
    cluster.add("n1");
    let body = r#"{"settings":{"index":{"number_of_shards":8}}}"#;
    assert_eq!(cluster.call(0, "PUT", "/logs", Some(body)).status, 200);
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
    for i in 0..300 {
        let q = format!(
            "{{\"query\":{{\"match\":{{\"msg\":\"w{}\"}}}},\"size\":10}}",
            i % 97
        );
        let r = cluster.call(0, "POST", "/logs/_search", Some(&q));
        assert_eq!(r.status, 200, "{}", r.text());
    }
    let peer_bytes = dir_bytes(&root.join("n1"));
    cluster.set("OSS_PAGE_CACHE_DIR", &root.join("n2").display().to_string());
    cluster.set("OSS_WARM_READY_PERCENT", &percent.to_string());
    cluster.set("OSS_WARM_BYTES_PER_SECOND", &rate.to_string());
    let started = Instant::now();
    let slot = cluster.add("n2");
    let mut not_ready = 0;
    let ready_ms = loop {
        let reply = cluster.call(slot, "GET", "/_serverless/ready", None);
        if reply.status == 200 {
            break started.elapsed().as_secs_f64() * 1000.0;
        }
        not_ready += 1;
        assert!(started.elapsed() < Duration::from_secs(300), "never ready");
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(cluster);
    let _ = std::fs::remove_dir_all(&root);
    Run {
        percent,
        peer_bytes,
        not_ready_samples: not_ready,
        early_ready: not_ready == 0,
        ready_ms,
    }
}

#[test]
#[ignore = "evidence: prints numbers, takes minutes"]
fn a_warming_joiner_is_let_in_at_the_ready_percent() {
    let docs = knob("WARM_READY_DOCS", 20_000);
    let rate = knob("WARM_READY_RATE", 524_288);
    println!("AC-7 docs={docs} rate={rate}B/s (emulator, Java latency profile; provisional)");
    for percent in [90, 50, 100] {
        let r = run(percent, docs, rate);
        println!(
            "AC-7 percent={} peer_tier={} not_ready_polls={} ready_ms={:.0} ready_on_first_poll={}",
            r.percent, r.peer_bytes, r.not_ready_samples, r.ready_ms, r.early_ready
        );
    }
}

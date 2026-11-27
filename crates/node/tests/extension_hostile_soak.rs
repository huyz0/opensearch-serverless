//! docs/specs/extension-analyzer AC-16: a soak of hostile modules through a
//! booted node's write path. Each module passes the create-time probe (the
//! text `probe`) and is hostile for any other text. Concurrent writers cycle
//! over one index per hostile kind and one good index. Every hostile write
//! must be refused with a typed `extension_` error, the good index must keep
//! accepting writes, and the process must not grow without bound.
//! Knobs: `OSS_SOAK_SECONDS` (default 5) and `OSS_SOAK_WRITERS` (8).
#![cfg(feature = "extensions")]
// ARITH: counts and byte sizes from a measurement; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "support/mod.rs"]
mod support;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

const IO: u32 = 4 * 1024 * 1024;

/// An analyzer whose answer to the text `probe` is the one token `probe`, and
/// whose call for any other text runs `body`, with the answer `len` bytes long.
fn module(body: &str, len: &str) -> Vec<u8> {
    build(body, len, 0)
}

fn build(body: &str, len: &str, always: u32) -> Vec<u8> {
    let d = format!(
        r#"{{"name":"ana","version":"1.0.0","abi":"1.0","io_bytes":{IO},"max_concurrent_calls":8,"limits":{{"fuel":2000000}},"points":[{{"id":"analyzer","version":"1.0.0"}}]}}"#
    );
    let mut canned = Vec::new();
    for w in [1u32, 5] {
        canned.extend_from_slice(&w.to_le_bytes());
    }
    canned.extend_from_slice(b"probe");
    for w in [0u32, 5, 1] {
        canned.extend_from_slice(&w.to_le_bytes());
    }
    let esc = |b: &[u8]| -> String { b.iter().map(|b| format!("\\{b:02x}")).collect() };
    let out_at = 65_536 + IO;
    wat::parse_str(format!(
        r#"(module
  (memory (export "memory") 130)
  (data (i32.const 0) "{d}")
  (data (i32.const 1000) "{o}")
  (global $h (mut i32) (i32.const 0))
  (func (export "oss_abi_version") (result i32) (i32.const 65536))
  (func (export "oss_describe") (result i32)
    (memory.copy (i32.const {out_at}) (i32.const 0) (i32.const {dl})) (i32.const {dl}))
  (func (export "oss_input_region") (result i32) (i32.const 65536))
  (func (export "oss_output_region") (result i32) (i32.const {out_at}))
  (func (export "oss_result_len") (result i32)
    (if (result i32) (global.get $h) (then {len}) (else (i32.const {n}))))
  (func (export "oss_call") (param i32 i32 i32) (result i32)
    (if (i32.or (i32.eq (i32.load8_u (i32.const 65544)) (i32.const 112)) (i32.const {always}))
      (then
        (memory.copy (i32.const {out_at}) (i32.const 1000) (i32.const {n}))
        (return (i32.const 0))))
    (global.set $h (i32.const 1))
    {body}))"#,
        d = esc(d.as_bytes()),
        o = esc(&canned),
        dl = d.len(),
        n = canned.len(),
    ))
    .expect("a valid module")
}

/// What each hostile module does after the probe, and a good one.
fn kinds() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (
            "spin",
            module("(loop $l (br $l)) (i32.const 0)", "i32.const 0"),
        ),
        ("trap", module("unreachable", "i32.const 0")),
        (
            "growth",
            module(
                "(drop (memory.grow (i32.const 60000))) (i32.const 0)",
                "i32.const 0",
            ),
        ),
        ("status", module("(i32.const 99)", "i32.const 0")),
        ("length", module("(i32.const 0)", "i32.const 2147483647")),
        ("empty", module("(i32.const 0)", "i32.const 0")),
    ]
}

fn boot(root: &std::path::Path) -> Node {
    let root = root.display().to_string();
    let lookup = |key: &str| match key {
        "OSS_NODE_ID" => Some("n1".to_owned()),
        "OSS_STORE" => Some("fs".to_owned()),
        "OSS_FS_ROOT" => Some(root.clone()),
        "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
        "OSS_EXTENSIONS_PERCENT" => Some("10".to_owned()),
        _ => None,
    };
    start(&NodeConfig::from_lookup(lookup).expect("valid")).expect("starts")
}

fn knob(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// The process's resident set in KiB, from `/proc`; 0 where there is none.
fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("VmRSS:"))
                .and_then(|v| v.split_whitespace().next().and_then(|n| n.parse().ok()))
        })
        .unwrap_or(0)
}

#[test]
fn a_soak_of_hostile_modules_through_the_write_path_is_typed_and_the_node_keeps_serving() {
    let seconds = knob("OSS_SOAK_SECONDS", 5);
    let writers = knob("OSS_SOAK_WRITERS", 8);
    let dir = TempDir::new("extension-hostile-soak");
    let node = boot(dir.path());
    let addr = node.addr();
    let ok = |r: support::Reply, what: &str| {
        assert!(
            (200..300).contains(&r.status),
            "{what}: {} {}",
            r.status,
            r.text()
        );
        r
    };
    ok(
        support::request(
            addr,
            "PUT",
            "/_cluster/settings",
            Some(br#"{"persistent":{"serverless.extensions.pins.allowed":true}}"#),
        ),
        "allow pins",
    );
    let mut all = kinds();
    all.push(("good", build("(i32.const 0)", "i32.const 0", 1)));
    let mut indexes = Vec::new();
    for (name, wasm) in &all {
        let up = ok(
            support::request(addr, "PUT", "/_serverless/extensions/modules", Some(wasm)),
            "upload",
        )
        .text();
        let hash = up
            .split("\"id\":\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .expect("a module id")
            .to_owned();
        let body = format!(
            r#"{{"settings":{{"index.analysis.analyzer.ext_a.type":"serverless_module","index.analysis.analyzer.ext_a.module":"{hash}","index.analysis.analyzer.ext_a.version":"1"}},"mappings":{{"properties":{{"t":{{"type":"text","analyzer":"ext_a"}}}}}}}}"#
        );
        ok(
            support::request(addr, "PUT", &format!("/h-{name}"), Some(body.as_bytes())),
            "create",
        );
        indexes.push(format!("h-{name}"));
    }
    let stop = AtomicBool::new(false);
    let refused = AtomicU64::new(0);
    let stray = std::sync::Mutex::new(Vec::<String>::new());
    let write = |index: &str| {
        support::request(
            addr,
            "POST",
            &format!("/{index}/_doc"),
            Some(br#"{"t":"xyz abc"}"#),
        )
    };
    // Opens the shards and the modules outside the measured window.
    for index in &indexes {
        let _ = write(index);
    }
    let before = rss_kib();
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let good_acked = AtomicU64::new(0);
    std::thread::scope(|s| {
        for w in 0..writers {
            let (indexes, stop, refused, stray, good_acked) =
                (&indexes, &stop, &refused, &stray, &good_acked);
            let write = &write;
            s.spawn(move || {
                let mut i = w as usize;
                while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                    let index = &indexes[i % indexes.len()];
                    i += 1;
                    let r = write(index);
                    let text = r.text();
                    let good = index == "h-good";
                    if (200..300).contains(&r.status) {
                        if good {
                            good_acked.fetch_add(1, Ordering::Relaxed);
                        } else {
                            stray
                                .lock()
                                .expect("lock")
                                .push(format!("{index} {} {text}", r.status));
                        }
                    } else if text.contains("extension_") {
                        refused.fetch_add(1, Ordering::Relaxed);
                    } else {
                        stray
                            .lock()
                            .expect("lock")
                            .push(format!("{index} {} {text}", r.status));
                    }
                }
            });
        }
    });
    let stray = stray.into_inner().expect("lock");
    assert!(
        stray.is_empty(),
        "untyped or accepted: {:#?}",
        &stray[..stray.len().min(5)]
    );
    assert!(
        refused.load(Ordering::Relaxed) > 0,
        "the hostile modules were called"
    );
    let after = rss_kib();
    println!(
        "{writers} writers, {seconds} s: {} typed refusals, {} good acks, RSS {before} KiB -> {after} KiB",
        refused.load(Ordering::Relaxed),
        good_acked.load(Ordering::Relaxed)
    );
    // The node still serves: a good write is acknowledged and the node is healthy.
    ok(write("h-good"), "a good write after the soak");
    ok(
        support::request(addr, "GET", "/_cluster/health", None),
        "health",
    );
    // Bounded growth: the loaded code and open shards, not a leak per call.
    if before > 0 {
        assert!(
            after.saturating_sub(before) < 512 * 1024,
            "resident set grew from {before} to {after} KiB"
        );
    }
}

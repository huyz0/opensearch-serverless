//! docs/specs/extension-analyzer AC-19: the write latency of a booted node
//! whose index pins a module that tokenises, against the same node's index
//! that uses the built-in analyzer. A measurement, run by hand:
//! `cargo test -p oss-node --release --all-features --test
//! extension_write_load -- --ignored --nocapture`. Knobs: `OSS_LOAD_WRITES`
//! (default 200) and `OSS_LOAD_WORDS` (default 40).
#![cfg(feature = "extensions")]
// ARITH: counts and microseconds from a measurement; an overflow panics the
// test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "support/mod.rs"]
mod support;

use std::time::Instant;

use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

const IO: usize = 4 << 20;

fn escaped(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:02x}")).collect()
}

/// An `analyzer` module that splits its one text on bytes up to a space and
/// answers each word as a token (the text's own bytes, its offsets, an
/// increment of one). It reads the block the host writes (a count of 1, a
/// length, the bytes) and lays the answer out as the ABI says.
fn tokenising() -> Vec<u8> {
    let d = format!(
        r#"{{"name":"words","version":"1.0.0","abi":"1.0","io_bytes":{IO},"max_concurrent_calls":8,"points":[{{"id":"analyzer","version":"1.0.0"}}]}}"#
    );
    let out_at = 65_536 + IO;
    wat::parse_str(format!(
        r#"(module
  (memory (export "memory") 130)
  (data (i32.const 0) "{d}")
  (global $len (mut i32) (i32.const 0))
  (func (export "oss_abi_version") (result i32) (i32.const 65536))
  (func (export "oss_describe") (result i32)
    (memory.copy (i32.const {out_at}) (i32.const 0) (i32.const {dl})) (i32.const {dl}))
  (func (export "oss_input_region") (result i32) (i32.const 65536))
  (func (export "oss_output_region") (result i32) (i32.const {out_at}))
  (func (export "oss_result_len") (result i32) (global.get $len))
  (func (export "oss_call") (param i32 i32 i32) (result i32)
    (local $i i32) (local $len i32) (local $start i32) (local $o i32)
    (local $cnt i32) (local $c i32) (local $tl i32)
    (local.set $len (i32.load (i32.const 65540)))
    (local.set $start (i32.const -1))
    (local.set $o (i32.const {tokens_at}))
    (block $done
      (loop $next
        (br_if $done (i32.gt_u (local.get $i) (local.get $len)))
        (local.set $c
          (if (result i32) (i32.lt_u (local.get $i) (local.get $len))
            (then (i32.load8_u (i32.add (i32.const 65544) (local.get $i))))
            (else (i32.const 32))))
        (if (i32.le_u (local.get $c) (i32.const 32))
          (then
            (if (i32.ge_s (local.get $start) (i32.const 0))
              (then
                (local.set $tl (i32.sub (local.get $i) (local.get $start)))
                (i32.store (local.get $o) (local.get $tl))
                (memory.copy (i32.add (local.get $o) (i32.const 4))
                  (i32.add (i32.const 65544) (local.get $start)) (local.get $tl))
                (i32.store (i32.add (local.get $o) (i32.add (i32.const 4) (local.get $tl)))
                  (local.get $start))
                (i32.store (i32.add (local.get $o) (i32.add (i32.const 8) (local.get $tl)))
                  (local.get $i))
                (i32.store (i32.add (local.get $o) (i32.add (i32.const 12) (local.get $tl)))
                  (i32.const 1))
                (local.set $o (i32.add (local.get $o) (i32.add (i32.const 16) (local.get $tl))))
                (local.set $cnt (i32.add (local.get $cnt) (i32.const 1)))
                (local.set $start (i32.const -1)))))
          (else
            (if (i32.lt_s (local.get $start) (i32.const 0))
              (then (local.set $start (local.get $i))))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $next)))
    (i32.store (i32.const {out_at}) (local.get $cnt))
    (global.set $len (i32.sub (local.get $o) (i32.const {out_at})))
    (i32.const 0)))"#,
        d = escaped(d.as_bytes()),
        dl = d.len(),
        tokens_at = out_at + 4,
    ))
    .expect("valid module")
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

fn knob(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn percentile(sorted: &[u128], q: usize) -> u128 {
    sorted[(sorted.len() * q / 100).saturating_sub(1)]
}

#[test]
#[ignore = "a measurement, run by hand"]
fn writes_to_an_index_that_pins_a_tokenising_module_against_the_built_in_analyzer() {
    let writes = knob("OSS_LOAD_WRITES", 200);
    let words = knob("OSS_LOAD_WORDS", 40);
    let dir = TempDir::new("extension-write-load");
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
    let upload = ok(
        support::request(
            addr,
            "PUT",
            "/_serverless/extensions/modules",
            Some(&tokenising()),
        ),
        "upload",
    )
    .text();
    let hash = upload
        .split("\"id\":\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .expect("a module id")
        .to_owned();
    let map = |analyzer: &str| format!(r#"{{"properties":{{"t":{{"type":"text"{analyzer}}}}}}}"#);
    ok(
        support::request(
            addr,
            "PUT",
            "/pinned",
            Some(
                format!(
                    r#"{{"settings":{{"index.analysis.analyzer.ext_words.type":"serverless_module","index.analysis.analyzer.ext_words.module":"{hash}","index.analysis.analyzer.ext_words.version":"1"}},"mappings":{}}}"#,
                    map(r#","analyzer":"ext_words""#)
                )
                .as_bytes(),
            ),
        ),
        "create pinned",
    );
    ok(
        support::request(
            addr,
            "PUT",
            "/builtin",
            Some(format!(r#"{{"mappings":{}}}"#, map("")).as_bytes()),
        ),
        "create built-in",
    );
    let text = "The Quick brown fox jumps over the lazy dog "
        .split(' ')
        .cycle()
        .take(words)
        .collect::<Vec<_>>()
        .join(" ");
    let body = format!(r#"{{"t":"{text}"}}"#);
    let mut report = Vec::new();
    for index in ["builtin", "pinned"] {
        // Opens the shard and the module, outside the clock.
        for _ in 0..20 {
            ok(
                support::request(
                    addr,
                    "POST",
                    &format!("/{index}/_doc"),
                    Some(body.as_bytes()),
                ),
                "warm",
            );
        }
        let mut micros = Vec::with_capacity(writes);
        for _ in 0..writes {
            let at = Instant::now();
            ok(
                support::request(
                    addr,
                    "POST",
                    &format!("/{index}/_doc"),
                    Some(body.as_bytes()),
                ),
                "write",
            );
            micros.push(at.elapsed().as_micros());
        }
        micros.sort_unstable();
        report.push(format!(
            "{index}: p50 {} us, p99 {} us, max {} us",
            percentile(&micros, 50),
            percentile(&micros, 99),
            micros[micros.len() - 1]
        ));
    }
    // The module ran: it keeps the case the built-in analyzer folds, so the
    // exact term `Quick` is in the pinned index and not in the built-in one.
    let found = |index: &str| {
        support::request(addr, "POST", &format!("/{index}/_refresh"), None);
        let target = format!("/{index}/_count");
        let query = br#"{"query":{"term":{"t":"Quick"}}}"#;
        let t0 = Instant::now();
        loop {
            let text = support::request(addr, "POST", &target, Some(query)).text();
            let n = text
                .split("\"count\":")
                .nth(1)
                .and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|n| n.parse::<usize>().ok());
            if n.is_some_and(|n| n > 0) || t0.elapsed().as_secs() > 5 {
                return n.unwrap_or(0);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };
    assert!(
        found("pinned") > 0,
        "the pinned index holds the module's terms"
    );
    assert_eq!(found("builtin"), 0, "the built-in analyzer folds the case");
    println!(
        "{writes} single-document writes of {words} words through a booted node (fs store, one client):\n{}",
        report.join("\n")
    );
}

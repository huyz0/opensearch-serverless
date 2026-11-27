//! docs/specs/extension-analyzer AC-11: through a booted node, a document
//! whose calls to its pinned module take more than ten of the module's
//! deadlines is refused as its own bulk item, nothing of it is written, and
//! the next item of the bulk is.
#![cfg(feature = "extensions")]
// ARITH: durations and counts of one measurement, small by the bounds the test
// asserts; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "support/mod.rs"]
mod support;

use std::time::{Duration, Instant};

use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;

const IO: usize = 4 << 20;
/// The deadline the module asks for, and so a tenth of a document's budget.
const DEADLINE_MS: u64 = 40;

fn escaped(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:02x}")).collect()
}

/// An `analyzer` module that spins for `rounds` and answers one token of one
/// byte at the start of its text, whatever the text.
fn spinning(rounds: u32) -> Vec<u8> {
    let d = format!(
        r#"{{"name":"spin","version":"1.0.0","abi":"1.0","io_bytes":{IO},"max_concurrent_calls":8,"limits":{{"deadline_ms":{DEADLINE_MS}}},"points":[{{"id":"analyzer","version":"1.0.0"}}]}}"#
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
    (local $i i32) (local $h i32)
    (block $done (loop $spin
      (br_if $done (i32.ge_u (local.get $i) (i32.const {rounds})))
      (local.set $h (i32.mul (i32.xor (local.get $h) (local.get $i)) (i32.const 16777619)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $spin)))
    (i32.store (i32.const 60000) (local.get $h))
    (i32.store (i32.const {out_at}) (i32.const 1))
    (i32.store (i32.add (i32.const {out_at}) (i32.const 4)) (i32.const 1))
    (i32.store8 (i32.add (i32.const {out_at}) (i32.const 8)) (i32.const 120))
    (i32.store (i32.add (i32.const {out_at}) (i32.const 9)) (i32.const 0))
    (i32.store (i32.add (i32.const {out_at}) (i32.const 13)) (i32.const 1))
    (i32.store (i32.add (i32.const {out_at}) (i32.const 17)) (i32.const 1))
    (global.set $len (i32.const 21))
    (i32.const 0)))"#,
        d = escaped(d.as_bytes()),
        dl = d.len(),
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

fn ok(r: support::Reply, what: &str) -> support::Reply {
    assert!(
        (200..300).contains(&r.status),
        "{what}: {} {}",
        r.status,
        r.text()
    );
    r
}

#[test]
fn a_document_whose_calls_outrun_ten_deadlines_is_refused_and_the_next_item_is_written() {
    let dir = TempDir::new("extension-item-cap");
    let node = boot(dir.path());
    let addr = node.addr();
    ok(
        support::request(
            addr,
            "PUT",
            "/_cluster/settings",
            Some(br#"{"persistent":{"serverless.extensions.pins.allowed":true}}"#),
        ),
        "allow pins",
    );
    let upload = |rounds: u32| -> String {
        let reply = ok(
            support::request(
                addr,
                "PUT",
                "/_serverless/extensions/modules",
                Some(&spinning(rounds)),
            ),
            "upload",
        )
        .text();
        reply
            .split("\"id\":\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .expect("a module id")
            .to_owned()
    };
    let create = |index: &str, hash: &str| {
        let settings = format!(
            r#"{{"settings":{{"index.analysis.analyzer.ext_spin.type":"serverless_module","index.analysis.analyzer.ext_spin.module":"{hash}","index.analysis.analyzer.ext_spin.version":"1"}},"mappings":{{"properties":{{"t":{{"type":"text","analyzer":"ext_spin"}}}}}}}}"#
        );
        ok(
            support::request(addr, "PUT", &format!("/{index}"), Some(settings.as_bytes())),
            "create",
        );
    };
    // One call's time, from `_analyze` (HTTP included, so an upper bound).
    let time_call = |index: &str| {
        let analyze = || {
            let at = Instant::now();
            ok(
                support::request(
                    addr,
                    "POST",
                    &format!("/{index}/_analyze"),
                    Some(br#"{"analyzer":"ext_spin","text":"abcd"}"#),
                ),
                "analyze",
            );
            at.elapsed()
        };
        analyze();
        let mut times: Vec<Duration> = (0..5).map(|_| analyze()).collect();
        times.sort();
        times[2]
    };
    // How fast this machine spins: a probe module, then the module whose call
    // takes about 6 ms here.
    const PROBE_ROUNDS: u32 = 400_000;
    // The most rounds the host's fuel (50,000,000, about a dozen units a round)
    // allows with a margin: a longer module fails its create-time probe.
    const MAX_ROUNDS: f64 = 3_000_000.0;
    create("probe", &upload(PROBE_ROUNDS));
    let probe = time_call("probe").as_secs_f64().max(0.000_05);
    let mut rounds = (f64::from(PROBE_ROUNDS) * 0.006 / probe).clamp(1_000.0, MAX_ROUNDS) as u32;
    create("capped", &upload(rounds));
    let mut target = "capped".to_owned();
    let mut call = time_call(&target);
    // A noisy probe can leave the first guess far off: rescale from the
    // module's own measured call, up to three times, until it is in range.
    for attempt in 0..3 {
        let in_range =
            call >= Duration::from_millis(3) && call < Duration::from_millis(DEADLINE_MS / 2);
        if in_range {
            break;
        }
        rounds = (f64::from(rounds) * 0.006 / call.as_secs_f64().max(0.000_05))
            .clamp(1_000.0, MAX_ROUNDS) as u32;
        target = format!("capped{attempt}");
        create(&target, &upload(rounds));
        call = time_call(&target);
    }
    assert!(
        call < Duration::from_millis(DEADLINE_MS / 2),
        "one call takes {call:?}, too near the {DEADLINE_MS} ms deadline to test the cap"
    );
    assert!(
        call > Duration::from_millis(2),
        "one call takes {call:?} at {rounds} rounds, too quick for the cap to be reached"
    );
    // Enough values that their calls take eight times the document's budget.
    let budget = Duration::from_millis(DEADLINE_MS * 10);
    let values = usize::try_from((budget * 8).as_micros() / call.as_micros().max(1))
        .expect("a count")
        .clamp(100, 20_000);
    let many = vec!["\"abcd\""; values].join(",");
    let bulk = format!(
        "{{\"index\":{{\"_index\":\"{target}\",\"_id\":\"big\"}}}}\n{{\"t\":[{many}]}}\n\
         {{\"index\":{{\"_index\":\"{target}\",\"_id\":\"small\"}}}}\n{{\"t\":\"abcd\"}}\n"
    );
    let reply = ok(
        support::request(addr, "POST", "/_bulk", Some(bulk.as_bytes())),
        "bulk",
    );
    let text = reply.text();
    assert!(text.contains("\"errors\":true"), "{text}");
    assert!(text.contains("extension_failed"), "{text}");
    assert!(text.contains("more than ten deadlines"), "{text}");
    assert!(text.contains("\"_id\":\"small\""), "{text}");
    // The refused document is not in the shard and the next one is.
    assert_eq!(
        support::request(addr, "GET", &format!("/{target}/_doc/big"), None).status,
        404
    );
    assert_eq!(
        support::request(addr, "GET", &format!("/{target}/_doc/small"), None).status,
        200
    );
}

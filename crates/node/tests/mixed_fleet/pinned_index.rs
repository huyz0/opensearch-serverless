//! docs/specs/extension-analyzer AC-19: the Java shell does not open an index that pins a module.

use super::*;

use oss_blob::{Credentials, S3Config, S3Store};
use oss_node::extension_store;

fn word(out: &mut Vec<u8>, w: u32) {
    out.extend_from_slice(&w.to_le_bytes());
}

/// An analyzer module that answers every text with the token `probe`.
fn module() -> Vec<u8> {
    const IO: u32 = 4 * 1024 * 1024;
    let mut out = Vec::new();
    for w in [1, 5] {
        word(&mut out, w);
    }
    out.extend_from_slice(b"probe");
    for w in [0, 5, 1] {
        word(&mut out, w);
    }
    let escaped = |bytes: &[u8]| -> String { bytes.iter().map(|b| format!("\\{b:02x}")).collect() };
    let d = format!(
        r#"{{"name":"ana","version":"1.0.0","abi":"1.0","io_bytes":{IO},"max_concurrent_calls":8,"points":[{{"id":"analyzer","version":"1.0.0"}}]}}"#
    );
    let n = out.len();
    let out_at = 65_536 + IO;
    wat::parse_str(format!(
        r#"(module
  (memory (export "memory") 130)
  (data (i32.const 0) "{d}")
  (data (i32.const 1000) "{o}")
  (global $len (mut i32) (i32.const 0))
  (func (export "oss_abi_version") (result i32) (i32.const 65536))
  (func (export "oss_describe") (result i32)
    (memory.copy (i32.const {out_at}) (i32.const 0) (i32.const {dl})) (i32.const {dl}))
  (func (export "oss_input_region") (result i32) (i32.const 65536))
  (func (export "oss_output_region") (result i32) (i32.const {out_at}))
  (func (export "oss_result_len") (result i32) (global.get $len))
  (func (export "oss_call") (param i32 i32 i32) (result i32)
    (memory.copy (i32.const {out_at}) (i32.const 1000) (i32.const {n}))
    (global.set $len (i32.const {n})) (i32.const 0)))"#,
        d = escaped(d.as_bytes()),
        o = escaped(&out),
        dl = d.len(),
    ))
    .expect("a valid module")
}

/// A Rust node creates an index that pins a module; a Java node on the same
/// bucket, started with the pins setting already true in the cluster config,
/// refuses to open it and keeps serving what it can.
#[test]
#[ignore = "needs OSS_JAVA_BIN and OSS_JAVA_CP"]
fn a_java_node_refuses_an_index_that_pins_a_module() {
    if std::env::var_os("OSS_JAVA_CP").is_none() || std::env::var_os("OSS_JAVA_BIN").is_none() {
        eprintln!("skipped: OSS_JAVA_BIN and OSS_JAVA_CP are not set");
        return;
    }
    let mut cluster = Cluster::new(TTL_MS, 1, false).expect("the emulated cluster");
    cluster.set("OSS_EXTENSIONS_PERCENT", "10");
    let (endpoint, bucket, region, access, secret) = cluster.store();
    let store = S3Store::new(S3Config::new(
        &endpoint,
        bucket,
        region,
        Credentials::new(access, secret),
    ))
    .expect("a store");
    let wasm = module();
    let hash = extension_store::hash_of(&wasm);
    extension_store::write_once(&store, "", &hash, &wasm).expect("the module is stored");

    let r1 = cluster.add("rust-1");
    let allow = cluster.call(
        r1,
        "PUT",
        "/_cluster/settings",
        Some(r#"{"persistent":{"serverless.extensions.pins.allowed":true}}"#),
    );
    assert_eq!(allow.status, 200, "{}", allow.text());
    let create = format!(
        r#"{{"settings":{{"index.analysis.analyzer.ext_stem.type":"serverless_module","index.analysis.analyzer.ext_stem.module":"{hash}","index.analysis.analyzer.ext_stem.version":"1"}},"mappings":{{"properties":{{"t":{{"type":"text","analyzer":"ext_stem"}}}}}}}}"#
    );
    let made = cluster.call(r1, "PUT", "/pinned", Some(&create));
    assert_eq!(made.status, 200, "{}", made.text());
    let plain = cluster.call(r1, "PUT", "/plain", Some("{}"));
    assert_eq!(plain.status, 200, "{}", plain.text());
    let rust = cluster.call(r1, "POST", "/pinned/_search", Some("{}"));
    assert_eq!(rust.status, 200, "rust opens its own pin: {}", rust.text());

    let java = JavaNode::start("java-pin", &temp("jpin"), &cluster, false);
    let health = call(java.addr, "GET", "/_cluster/health", None);
    assert_eq!(health.status, 200, "{}", health.text());
    // With the Rust node gone and its lease lapsed, the Java node takes the
    // shard of an index that pins nothing, and never the one that pins a module.
    cluster.kill(r1);
    let bulk = |index: &str| {
        call(
            java.addr,
            "POST",
            "/_bulk",
            Some(&format!(
                "{{\"index\":{{\"_index\":\"{index}\",\"_id\":\"1\"}}}}\n{{\"t\":\"jumps\"}}\n"
            )),
        )
    };
    let started = Instant::now();
    loop {
        let reply = bulk("plain");
        eprintln!("JAVA PLAIN {}", reply.text());
        if reply.text().contains("\"errors\":false") {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(HANDOVER_SLACK_SECS),
            "the control index was never taken over: {}",
            reply.text()
        );
        std::thread::sleep(Duration::from_millis(500));
    }
    let refused = bulk("pinned");
    eprintln!("JAVA PINNED {}", refused.text());
    assert!(
        refused.text().contains("\"errors\":true"),
        "{}",
        refused.text()
    );
    assert!(
        refused.text().contains("not_the_writer"),
        "the Java node holds no shard of the pinned index: {}",
        refused.text()
    );
    java.kill();
}

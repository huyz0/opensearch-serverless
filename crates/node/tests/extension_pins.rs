//! docs/specs/extension-analyzer AC-3, AC-4, AC-5: a module-backed policy
//! reads the pinned module once, checks it can be an analyzer, probes it and
//! resolves its limits.
#![cfg(feature = "extensions")]

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_ext::pin::Pin;
use oss_ext_wasm::{Host, HostConfig};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::extension_pins::ModulePins;
use oss_node::extension_store;
use oss_node::pin_policy::PinPolicy;
use oss_testkit::budget;

const IO: u32 = 4 * 1024 * 1024;

fn word(out: &mut Vec<u8>, w: u32) {
    out.extend_from_slice(&w.to_le_bytes());
}

/// The output for the one text `probe`: one token, the text itself.
fn canned() -> Vec<u8> {
    let mut o = Vec::new();
    word(&mut o, 1);
    word(&mut o, 5);
    o.extend_from_slice(b"probe");
    word(&mut o, 0);
    word(&mut o, 5);
    word(&mut o, 1);
    o
}

fn escaped(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:02x}")).collect()
}

/// An analyzer module (`point`), with or without an import and with a
/// descriptor limit on fuel.
fn module(point: &str, import: bool, fuel: Option<u64>) -> Vec<u8> {
    let limits = fuel.map_or(String::new(), |f| format!(r#","limits":{{"fuel":{f}}}"#));
    let d = format!(
        r#"{{"name":"ana","version":"1.0.0","abi":"1.0","io_bytes":{IO},"max_concurrent_calls":8{limits},"points":[{{"id":"{point}","version":"1.0.0"}}]}}"#
    );
    let out = canned();
    let n = out.len();
    let out_at = 65_536 + IO;
    let import = if import {
        r#"(import "oss" "log" (func (param i32 i32)))"#
    } else {
        ""
    };
    wat::parse_str(format!(
        r#"(module {import}
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
    .expect("valid module")
}

struct Rig {
    store: Arc<Emulator>,
    policy: ModulePins,
}

fn rig(allow: bool) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let budget = MemoryBudget::new(
        &BudgetConfig::new(256 << 20)
            .with_pool(Pool::RequestBodies, 32 << 20)
            .and_then(|c| c.with_pool(Pool::Extensions, 128 << 20))
            .and_then(|c| c.with_pool(Pool::OpenIndexes, 16 << 20))
            .expect("fits"),
    );
    let policy = ModulePins::new(
        Arc::clone(&store) as Arc<dyn BlobStore>,
        "",
        Host::new(HostConfig::default()),
        budget,
        Arc::new(move |_| allow),
    );
    Rig { store, policy }
}

fn put(r: &Rig, wasm: &[u8]) -> String {
    let hash = extension_store::hash_of(wasm);
    extension_store::write_once(r.store.as_ref(), "", &hash, wasm).expect("stored");
    hash
}

fn pin(hash: &str) -> Pin {
    Pin {
        name: "ext_a".into(),
        module: hash.into(),
        version: 1,
    }
}

#[test]
fn a_good_module_resolves_the_host_defaults_lowered_by_its_descriptor() {
    let r = rig(true);
    let hash = put(&r, &module("analyzer", false, Some(1_000)));
    let gets = r.store.meter().count(S3Op::Get);
    let got = r.policy.check(&pin(&hash)).expect("accepted");
    let host = HostConfig::default();
    assert_eq!(got.fuel, 1_000, "the descriptor lowers fuel");
    assert_eq!(got.memory_bytes, host.memory_bytes);
    assert_eq!(got.deadline_ms, host.deadline_ms);
    budget::check(
        "extension.pin_check.requests",
        r.store.meter().count(S3Op::Get) - gets,
        "one GET reads the module; the probe makes none",
    );
}

#[test]
fn a_module_that_cannot_be_an_analyzer_is_refused_in_words() {
    let r = rig(true);
    let absent = "f".repeat(64);
    let e = r.policy.check(&pin(&absent)).expect_err("absent");
    assert!(e.contains("not in the store"), "{e}");
    let h = put(&r, &module("echo", false, None));
    assert!(
        r.policy
            .check(&pin(&h))
            .expect_err("point")
            .contains("analyzer")
    );
    let h = put(&r, &module("analyzer", true, None));
    assert!(
        r.policy
            .check(&pin(&h))
            .expect_err("import")
            .contains("imports")
    );
}

#[test]
fn authority_is_the_policys_own_answer() {
    assert!(rig(true).policy.authorised(Some("admin")));
    assert!(!rig(false).policy.authorised(None));
}

// docs/specs/extension-analyzer AC-9: the cluster setting gates a pin.
#[test]
fn a_pin_waits_for_the_cluster_setting_and_follows_it_back_off() {
    let r = rig(true);
    let policy = ModulePins::new(
        Arc::clone(&r.store) as Arc<dyn BlobStore>,
        "",
        Host::new(HostConfig::default()),
        MemoryBudget::new(&BudgetConfig::new(64 << 20)),
        Arc::new(|_| true),
    )
    .gated_by_the_cluster_setting()
    .expect("gate");
    let config = oss_meta::ClusterConfig::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "")
        .expect("config");
    let mut buf = vec![0u8; 1 << 20];
    let key = "serverless.extensions.pins.allowed";
    let gets = r.store.meter().count(S3Op::Get);
    assert!(policy.allowed().is_err(), "no register: off");
    budget::check(
        "extension.pin_allowed.requests",
        r.store.meter().count(S3Op::Get) - gets,
        "one GET of the cluster register",
    );
    config
        .update(&[(key, Some("true"))], &mut buf)
        .expect("set");
    assert!(policy.allowed().is_ok(), "true: on");
    config
        .update(&[(key, Some("false"))], &mut buf)
        .expect("set");
    assert!(policy.allowed().is_err(), "false: off again");
    // A policy not gated allows, for a caller that checks elsewhere.
    assert!(r.policy.allowed().is_ok());
}

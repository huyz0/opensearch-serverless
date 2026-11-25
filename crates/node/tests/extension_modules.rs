//! docs/specs/extension-analyzer AC-5, AC-12, AC-14, AC-15: a node opening
//! an index with pins loads each distinct module once, runs it under the
//! limits the descriptor recorded, and stretches them for a replay.
#![cfg(all(feature = "extensions", feature = "writer"))]

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_ext::pin::{Pin, ResolvedLimits, render};
use oss_ext_wasm::{Host, HostConfig};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::extension_modules::ModuleSource;
use oss_node::extension_store;
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

const HASH_ABSENT: &str = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

struct Rig {
    store: Arc<Emulator>,
    source: ModuleSource,
}

fn rig(factor: u64) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let budget = MemoryBudget::new(
        &BudgetConfig::new(256 << 20)
            .with_pool(Pool::RequestBodies, 32 << 20)
            .and_then(|c| c.with_pool(Pool::Extensions, 128 << 20))
            .and_then(|c| c.with_pool(Pool::OpenIndexes, 16 << 20))
            .expect("fits"),
    );
    let source = ModuleSource::new(
        Arc::clone(&store) as Arc<dyn BlobStore>,
        "",
        Host::new(HostConfig::default()),
        budget,
        factor,
    );
    Rig { store, source }
}

fn put(r: &Rig, wasm: &[u8]) -> String {
    let hash = extension_store::hash_of(wasm);
    extension_store::write_once(r.store.as_ref(), "", &hash, wasm).expect("stored");
    hash
}

/// The settings JSON a descriptor with these pins carries.
fn settings(pins: &[(&str, &str, u64)]) -> Vec<u8> {
    let pins: Vec<(Pin, ResolvedLimits)> = pins
        .iter()
        .map(|(name, hash, fuel)| {
            (
                Pin {
                    name: (*name).into(),
                    module: (*hash).into(),
                    version: 1,
                },
                ResolvedLimits {
                    fuel: *fuel,
                    memory_bytes: 16 << 20,
                    deadline_ms: 10_000,
                },
            )
        })
        .collect();
    let mut out = vec![b'{'];
    out.extend_from_slice(br#""index.refresh_interval":"1s""#);
    for (k, v) in render(&pins) {
        out.extend_from_slice(format!(r#",{k:?}:{v:?}"#).as_bytes());
    }
    out.push(b'}');
    out
}

#[test]
fn an_index_without_pins_loads_nothing_and_reads_nothing() {
    let r = rig(16);
    let gets = r.store.meter().count(S3Op::Get);
    let set = r
        .source
        .open(br#"{"index.refresh_interval":"1s"}"#, "t")
        .expect("opens");
    assert!(set.is_empty());
    assert_eq!(r.store.meter().count(S3Op::Get), gets);
}

// AC-15
#[test]
fn two_pins_of_one_module_cost_one_get_and_each_analyses() {
    let r = rig(16);
    let hash = put(&r, &module("analyzer", false, None));
    let gets = r.store.meter().count(S3Op::Get);
    let set = r
        .source
        .open(
            &settings(&[("ext_a", &hash, 10_000_000), ("ext_b", &hash, 10_000_000)]),
            "t",
        )
        .expect("opens");
    budget::check(
        "extension.open.requests",
        r.store.meter().count(S3Op::Get) - gets,
        "one GET a distinct module, none a pin",
    );
    assert_eq!(set.len(), 2);
    for (name, analyzer) in set.analyzers() {
        let tokens = analyzer.analyze("probe").expect(name);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].term, "probe");
    }
}

// AC-14
#[test]
fn a_module_that_is_not_in_the_store_refuses_the_open_in_words() {
    let r = rig(16);
    let e = r
        .source
        .open(&settings(&[("ext_a", HASH_ABSENT, 1_000)]), "t")
        .err()
        .expect("refused");
    assert!(e.contains("extension_unavailable"), "{e}");
    assert!(e.contains(HASH_ABSENT), "{e}");
}

/// The least fuel at which the module answers, found by doubling.
fn least_fuel(r: &Rig, hash: &str) -> u64 {
    let mut fuel = 1;
    loop {
        let set = r
            .source
            .open(&settings(&[("ext_a", hash, fuel)]), "t")
            .expect("opens");
        let (_, analyzer) = set.analyzers().next().expect("one");
        if analyzer.analyze("probe").is_ok() {
            return fuel;
        }
        fuel = fuel.saturating_mul(2);
        assert!(fuel < 1 << 40, "the module never answers");
    }
}

// AC-5, AC-12
#[test]
fn a_write_runs_under_the_recorded_fuel_and_a_replay_under_it_stretched() {
    let r = rig(4);
    let hash = put(&r, &module("analyzer", false, None));
    // Half of what a call needs is too little for a write and, times the
    // factor, enough for a replay.
    let short = least_fuel(&r, &hash) / 2;
    let set = r
        .source
        .open(&settings(&[("ext_a", &hash, short)]), "t")
        .expect("opens");
    let (_, analyzer) = set.analyzers().next().expect("one");
    assert!(
        analyzer.analyze("probe").is_err(),
        "{short} units are not enough"
    );
    set.replaying(true);
    assert!(
        analyzer.analyze("probe").is_ok(),
        "a replay has the record times the factor"
    );
    set.replaying(false);
    assert!(analyzer.analyze("probe").is_err());
}

// AC-12
#[test]
fn a_replay_that_exceeds_even_the_stretched_fuel_is_recorded_as_a_failure() {
    let r = rig(4);
    let hash = put(&r, &module("analyzer", false, None));
    // An eighth of what a call needs is too little at four times as well.
    let tiny = (least_fuel(&r, &hash) / 8).max(1);
    let set = r
        .source
        .open(&settings(&[("ext_a", &hash, tiny)]), "t")
        .expect("opens");
    let (_, analyzer) = set.analyzers().next().expect("one");
    set.replaying(true);
    assert!(analyzer.analyze("probe").is_err());
    set.replaying(false);
    let why = set.replay_failure().expect("the failure is kept");
    assert!(why.contains(&hash), "{why}");
    assert_eq!(set.replay_failures(), 1);
    assert!(set.replay_failure().is_none(), "taking it clears it");
    // A live write that fails is not a replay failure.
    assert!(analyzer.analyze("probe").is_err());
    assert_eq!(set.replay_failures(), 1);
}

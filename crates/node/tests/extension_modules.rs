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
    module_with(point, import, fuel, "")
}

/// [`module`] whose call first runs `before`.
fn module_with(point: &str, import: bool, fuel: Option<u64>, before: &str) -> Vec<u8> {
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
    {before}
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
    budget: MemoryBudget,
}

fn rig(factor: u64) -> Rig {
    rig_sized(factor, 128 << 20)
}

/// A rig whose extensions pool holds `extensions` bytes.
fn rig_sized(factor: u64, extensions: usize) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let budget = MemoryBudget::new(
        &BudgetConfig::new(256 << 20)
            .with_pool(Pool::RequestBodies, 32 << 20)
            .and_then(|c| c.with_pool(Pool::Extensions, extensions))
            .and_then(|c| c.with_pool(Pool::OpenIndexes, 16 << 20))
            .expect("fits"),
    );
    let source = ModuleSource::new(
        Arc::clone(&store) as Arc<dyn BlobStore>,
        "",
        Host::new(HostConfig::default()),
        budget.clone(),
        factor,
    );
    Rig {
        store,
        source,
        budget,
    }
}

fn put(r: &Rig, wasm: &[u8]) -> String {
    let hash = extension_store::hash_of(wasm);
    extension_store::write_once(r.store.as_ref(), "", &hash, wasm).expect("stored");
    hash
}

/// The settings JSON a descriptor with these pins carries.
fn settings(pins: &[(&str, &str, u64)]) -> Vec<u8> {
    settings_in(pins, 10_000)
}

/// [`settings`] with the deadline each pin records.
fn settings_in(pins: &[(&str, &str, u64)], deadline_ms: u64) -> Vec<u8> {
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
                    deadline_ms,
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

fn refusal(r: &Rig, hash: &str) -> String {
    r.source
        .open(&settings(&[("ext_a", hash, 1_000)]), "t")
        .map(drop)
        .expect_err("refused")
}

// AC-14
#[test]
fn a_module_that_does_not_match_its_hash_or_cannot_be_an_analyzer_refuses_the_open() {
    let r = rig(16);
    // The bytes of one module stored under the hash of another.
    let claimed = extension_store::hash_of(&module("analyzer", false, None));
    extension_store::write_once(
        r.store.as_ref(),
        "",
        &claimed,
        &module("analyzer", false, Some(7)),
    )
    .expect("stored");
    let e = refusal(&r, &claimed);
    assert!(e.starts_with("extension_unavailable"), "{e}");
    assert!(e.contains("does not match its hash"), "{e}");
    // A module with no `analyzer` point, and bytes that are no module.
    let other = put(&r, &module("tokenizer", false, None));
    let e = refusal(&r, &other);
    assert!(e.starts_with("extension_unavailable"), "{e}");
    let junk = put(&r, b"not wasm");
    let e = refusal(&r, &junk);
    assert!(e.starts_with("extension_unavailable"), "{e}");
}

// AC-14
#[test]
fn a_pool_too_small_for_the_compile_is_a_retryable_refusal_that_is_not_kept() {
    let r = rig(16);
    let hash = put(&r, &module("analyzer", false, None));
    // Every byte but one MiB of the extensions pool is taken elsewhere.
    let room = r.budget.limit(Pool::Extensions);
    let taken = r
        .budget
        .try_reserve(Pool::Extensions, room - (1 << 20))
        .expect("reserved");
    let e = refusal(&r, &hash);
    assert!(e.starts_with("extension_unavailable"), "{e}");
    assert!(e.contains(oss_node::search_modules::NO_CAPACITY), "{e}");
    // Capacity passes: once the room is back, the same module opens.
    drop(taken);
    r.source
        .open(&settings(&[("ext_a", &hash, 1_000)]), "t")
        .expect("opens when there is room");
}

// AC-14, AC-18
#[test]
fn a_refused_module_is_not_read_again_for_thirty_seconds() {
    let now = Arc::new(std::sync::atomic::AtomicU64::new(1_000));
    let mut r = rig(16);
    let clock = Arc::clone(&now);
    r.source = r.source.with_clock(Arc::new(move || {
        clock.load(std::sync::atomic::Ordering::SeqCst)
    }));
    let requests = |r: &Rig| r.store.meter().count(S3Op::Get) + r.store.meter().count(S3Op::Head);
    let first = refusal(&r, HASH_ABSENT);
    let spent = requests(&r);
    assert!(spent > 0, "the first open reads");
    now.fetch_add(29_999, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(refusal(&r, HASH_ABSENT), first);
    assert_eq!(requests(&r), spent, "no request within 30 s");
    now.fetch_add(2, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(refusal(&r, HASH_ABSENT), first);
    assert!(requests(&r) > spent, "30 s on, the store is asked again");
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

// AC-11: a document's calls may take ten of the recorded deadlines in all
// when it is written live, and no cap holds a replay or the overlay build.
#[test]
fn a_live_write_has_ten_deadlines_to_a_document_and_a_replay_has_no_cap() {
    let r = rig(4);
    let hash = put(&r, &module("analyzer", false, None));
    let set = r
        .source
        .open(
            &settings_in(&[("ext_a", &hash, least_fuel(&r, &hash) * 4)], 250),
            "t",
        )
        .expect("opens");
    let (_, analyzer) = set.analyzers().next().expect("one");
    assert_eq!(
        analyzer.item_budget(),
        Some(std::time::Duration::from_millis(2_500))
    );
    set.replaying(true);
    assert_eq!(analyzer.item_budget(), None);
    set.replaying(false);
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

// AC-8: the overlay's modules run under the record exactly and are not
// refused by the tenant's share that live calls are held to.
#[test]
fn the_overlays_modules_are_not_refused_by_the_tenants_share() {
    let r = rig(4);
    let hash = put(
        &r,
        &module_with("analyzer", false, None, "(loop $l (br $l))"),
    );
    let set = Arc::new(
        r.source
            .open(&settings_in(&[("ext_a", &hash, 1 << 40)], 1_500), "t")
            .expect("opens"),
    );
    let live = |set: &Arc<oss_node::extension_modules::ModuleSet>| {
        let set = Arc::clone(set);
        std::thread::spawn(move || {
            let (_, a) = set.analyzers().next().expect("one");
            a.analyze("probe").map(drop)
        })
    };
    let spinners = [live(&set), live(&set)];
    std::thread::sleep(std::time::Duration::from_millis(300));
    let (_, a) = set.analyzers().next().expect("one");
    let busy = a.analyze("probe").unwrap_err();
    assert_eq!(
        busy.kind(),
        oss_ext::FailureKind::Busy,
        "the share is spent"
    );
    let tokens: &dyn oss_reader::pinned::PinnedTokens = set.as_ref();
    let (_, overlay) = tokens.modules().into_iter().next().expect("one");
    let e = overlay.analyze("probe").unwrap_err();
    assert_ne!(
        e.kind(),
        oss_ext::FailureKind::Busy,
        "an overlay build is let through and fails on its own fuel"
    );
    for s in spinners {
        assert!(s.join().expect("joined").is_err());
    }
}

/// The writer's shard over `dir`, a text field `t` that pins `ext_a` when
/// `pinned` and is built in otherwise.
fn shard_of<'d>(
    dir: &'d lucene_store::ByteBuffersDirectory,
    pinned: Option<Arc<dyn oss_ext::analyzer::Analyze>>,
) -> oss_writer::index::Shard<'d> {
    use oss_writer::mapping::{Mapping, ParseOptions};
    let analyzer = if pinned.is_some() {
        r#","analyzer":"ext_a""#
    } else {
        ""
    };
    let json = format!(r#"{{"properties":{{"t":{{"type":"text"{analyzer}}}}}}}"#);
    let mapping = Mapping::parse_pinned(json.as_bytes(), ParseOptions::default()).expect("mapping");
    let (writer, derived) = oss_writer::index::knn::open_writer(dir, &mapping).expect("writer");
    let shard = oss_writer::index::Shard::new(dir, writer, mapping, 1, Box::new(|| 0))
        .expect("shard")
        .with_derived_source(derived);
    match pinned {
        Some(m) => shard.with_module("ext_a", m),
        None => shard,
    }
}

/// Run by hand: `cargo test -p oss-node --release --all-features --test
/// extension_modules -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, run by hand"]
fn the_cost_of_a_pinned_write_against_the_built_in_analyzer() {
    let r = rig(4);
    let hash = put(&r, &module("analyzer", false, None));
    let set = r
        .source
        .open(&settings(&[("ext_a", &hash, 50_000_000)]), "t")
        .expect("opens");
    let (_, module) = set.analyzers().next().expect("one");
    let text = "the quick brown fox jumps over the lazy dog ".repeat(4);
    let source = format!(r#"{{"t":"{text}"}}"#);
    let mut report = Vec::new();
    for (name, pinned) in [("built-in", None), ("pinned", Some(module))] {
        let dir = lucene_store::ByteBuffersDirectory::new();
        let mut shard = shard_of(&dir, pinned);
        let mut nanos: Vec<u128> = Vec::new();
        for n in 0..5_200 {
            let at = std::time::Instant::now();
            shard
                .index(
                    &format!("d{n}"),
                    source.as_bytes(),
                    false,
                    &oss_writer::index::Expect::default(),
                )
                .expect("indexed");
            if n >= 200 {
                nanos.push(at.elapsed().as_nanos());
            }
        }
        nanos.sort_unstable();
        let at = |q: usize| nanos[nanos.len() * q / 100 - 1] / 1000;
        report.push(format!(
            "{name}: p50 {} us, p99 {} us, max {} us",
            at(50),
            at(99),
            nanos[nanos.len() - 1] / 1000
        ));
    }
    println!(
        "5,000 writes of a 176-byte text after 200 warm-up:\n{}",
        report.join("\n")
    );
}

// AC-18: an open index's module is resident. Another module that cannot be
// charged is refused, the held one stays charged and answers, and neither the
// refusal nor the call goes to the store.
#[test]
fn an_open_indexs_module_stays_resident_under_pool_pressure() {
    let r = rig(16);
    let held = put(&r, &module("analyzer", false, None));
    let other = put(&r, &module("analyzer", false, Some(7)));
    let set = r
        .source
        .open(&settings(&[("ext_a", &held, 10_000_000)]), "t")
        .expect("opens");
    let charged = r.budget.in_use(Pool::Extensions);
    assert!(charged > 0, "the open holds its module's code");

    // Everything but one MiB is taken elsewhere: a second module cannot open.
    let room = r.budget.available(Pool::Extensions);
    let taken = r
        .budget
        .try_reserve(Pool::Extensions, room - (1 << 20))
        .expect("reserved");
    let e = r
        .source
        .open(&settings(&[("ext_b", &other, 1_000)]), "t")
        .map(drop)
        .expect_err("refused");
    assert!(e.contains(oss_node::search_modules::NO_CAPACITY), "{e}");
    assert!(
        r.budget.in_use(Pool::Extensions) >= charged + (room - (1 << 20)),
        "the held module is not evicted to make room"
    );

    drop(taken);
    let requests = |r: &Rig| r.store.meter().count(S3Op::Get) + r.store.meter().count(S3Op::Head);
    let before = requests(&r);
    let (_, analyzer) = set.analyzers().next().expect("one");
    for _ in 0..3 {
        assert_eq!(analyzer.analyze("probe").expect("answers")[0].term, "probe");
    }
    assert_eq!(requests(&r), before, "a call reads nothing from the store");
}

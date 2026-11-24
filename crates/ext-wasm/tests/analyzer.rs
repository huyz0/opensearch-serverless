//! A module as the `analyzer` point (docs/specs/extension-analyzer AC-2, AC-3).

mod common;

use std::sync::Arc;

use common::{Parts, build};
use oss_ext::FailureKind;
use oss_ext::analyzer::Token;
use oss_ext_wasm::{Host, HostConfig, LoadError, Module, PinRefusal, Unbounded};

const AT: u32 = 1000;

fn word(out: &mut Vec<u8>, w: u32) {
    out.extend_from_slice(&w.to_le_bytes());
}

/// The output for two one-token texts, `ab` and `cd`.
fn canned() -> Vec<u8> {
    let mut o = Vec::new();
    for t in [b"ab", b"cd"] {
        word(&mut o, 1);
        word(&mut o, 2);
        o.extend_from_slice(t);
        word(&mut o, 0);
        word(&mut o, 2);
        word(&mut o, 1);
    }
    o
}

fn descriptor(io: u32, point: &str) -> String {
    format!(
        r#"{{"name":"ana","version":"1.0.0","abi":"1.0","io_bytes":{io},"max_concurrent_calls":8,"points":[{{"id":"{point}","version":"1.0.0"}}]}}"#
    )
}

fn escaped(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:02x}")).collect()
}

/// A module that fails unless it is handed a block of two texts, and answers
/// `output` whatever they say.
fn analyzer(output: &[u8], io: u32, point: &str, bare: bool) -> Vec<u8> {
    let n = output.len();
    let extra = format!(r#"(data (i32.const {AT}) "{}")"#, escaped(output));
    let body = format!(
        "(if (i32.ne (i32.load (i32.const 65536)) (i32.const 2)) (then unreachable)) \
         (memory.copy (i32.const 131072) (i32.const {AT}) (i32.const {n})) \
         (global.set $len (i32.const {n})) (i32.const 0)"
    );
    build(&Parts {
        bare,
        descriptor: descriptor(io, point),
        body: &body,
        extra: &extra,
        ..Parts::default()
    })
}

fn load(wasm: &[u8]) -> Result<Module, LoadError> {
    Host::new(HostConfig::default()).load(wasm, Arc::new(Unbounded))
}

#[test]
fn a_block_goes_in_and_its_tokens_come_back_validated() {
    let m = load(&analyzer(&canned(), 1024, "analyzer", true)).expect("loads");
    let got = m.analyze_block("t", &["ab", "cd"]).expect("analyzes");
    let tok = |s: &str| Token {
        term: s.into(),
        start: 0,
        end: 2,
        increment: 1,
    };
    assert_eq!(got, vec![vec![tok("ab")], vec![tok("cd")]]);
    // `analyze` is the same call for a block this small.
    assert_eq!(m.analyze("t", &["ab", "cd"]).expect("analyzes"), got);
}

#[test]
fn a_block_the_module_does_not_like_is_a_trap_not_an_answer() {
    let m = load(&analyzer(&canned(), 1024, "analyzer", true)).expect("loads");
    // The module traps unless it was handed two texts.
    let e = m.analyze_block("t", &["ab"]).expect_err("trapped");
    assert_eq!(e.kind(), FailureKind::Trap);
}

#[test]
fn an_output_the_contract_forbids_is_refused_after_the_call() {
    let mut bad = canned();
    bad.push(0);
    let m = load(&analyzer(&bad, 1024, "analyzer", true)).expect("loads");
    let e = m.analyze_block("t", &["ab", "cd"]).expect_err("refused");
    assert_eq!((e.kind(), e.http_status()), (FailureKind::Malformed, 502));
    // An over-long block is refused before the module is called.
    let e = m.analyze_block("t", &[]).expect_err("empty");
    assert_eq!(e.kind(), FailureKind::Limit);
}

#[test]
fn a_module_without_the_point_is_unsupported() {
    let m = load(&analyzer(&canned(), 1024, "echo", true)).expect("loads");
    let e = m.analyze_block("t", &["ab", "cd"]).expect_err("no point");
    assert_eq!(e.kind(), FailureKind::Unsupported);
}

// docs/specs/extension-analyzer AC-3
#[test]
fn a_module_with_imports_or_no_point_or_small_regions_cannot_be_pinned() {
    let big = 4 * 1024 * 1024;
    let ok = |imports: bool, io: u32, point: &str| {
        let parts = Parts {
            bare: !imports,
            descriptor: descriptor(io, point),
            memory: "130",
            input: 65_536,
            output: 65_536 + 4 * 1024 * 1024,
            ..Parts::default()
        };
        load(&build(&parts)).expect("loads")
    };
    assert_eq!(ok(false, big, "analyzer").check_analyzer_pin(1), Ok(()));
    assert_eq!(
        ok(false, big, "analyzer").check_analyzer_pin(2),
        Err(PinRefusal::NoPoint(2))
    );
    assert_eq!(
        ok(false, big, "echo").check_analyzer_pin(1),
        Err(PinRefusal::NoPoint(1))
    );
    assert_eq!(
        ok(true, big, "analyzer").check_analyzer_pin(1),
        Err(PinRefusal::Imports(2))
    );
    let small = ok(false, big - 1, "analyzer");
    assert_eq!(
        small.check_analyzer_pin(1),
        Err(PinRefusal::RegionTooSmall {
            declared: big - 1,
            needed: big
        })
    );
}

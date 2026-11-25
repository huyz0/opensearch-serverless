//! The hostile soak (docs/specs/extension-analyzer AC-16): many threads and tenants
//! call every kind of hostile module over one host and one small pool.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use common::{Handle, Parts, Pool, build, with_body};
use oss_ext::FailureKind;
use oss_ext_wasm::{Host, HostConfig, Module};

const MODULES: [&str; 8] = [
    "(loop $l (br $l)) (i32.const 0)",
    "(drop (memory.grow (i32.const 100000))) (i32.const 0)",
    "(unreachable)",
    "(global.set $len (i32.const -1)) (i32.const 0)",
    "(i32.const -99)",
    "(i32.const -4)",
    "(i32.const 0)",
    "(memory.copy (i32.const 131072) (i32.const 65536) (local.get 1)) (global.set $len (local.get 1)) (i32.const 0)",
];

// docs/specs/extension-analyzer AC-16
#[test]
fn a_soak_of_hostile_modules_fails_each_call_typed_and_leaves_the_pool_empty() {
    let formula = 64 * 1024 * 1024 + 2 * u64::from(common::IO) + 1024 * 1024;
    let wasms: Vec<Vec<u8>> = MODULES.iter().map(|b| with_body(b)).collect();
    let code: u64 = wasms
        .iter()
        .map(|w| u64::try_from(w.len()).unwrap() * 16)
        .sum();
    // Room for four calls at once, so a busy answer is common.
    let budget = code + 4 * formula;
    let pool = Pool::of(budget);
    let host = Host::new(HostConfig {
        fuel: 2_000_000,
        deadline_ms: 200,
        ..HostConfig::default()
    });
    let modules: Arc<Vec<Module>> = Arc::new(
        wasms
            .iter()
            .map(|w| {
                host.load(w, Arc::new(Handle(Arc::clone(&pool))))
                    .expect("loads")
            })
            .collect(),
    );
    let threads: Vec<_> = (0..16_u32)
        .map(|t| {
            let modules = Arc::clone(&modules);
            std::thread::spawn(move || {
                let mut kinds = Vec::new();
                for i in 0..40_u32 {
                    let m =
                        &modules[usize::try_from(t.wrapping_mul(7).wrapping_add(i) % 8).unwrap()];
                    let tenant = format!("t{}", (t + i) % 5);
                    match m.call(&tenant, 0, b"abc") {
                        Ok(out) => assert!(out.is_empty() || out == b"abc"),
                        Err(e) => kinds.push(e.kind()),
                    }
                }
                kinds
            })
        })
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    for t in threads {
        for k in t.join().expect("no thread panicked") {
            assert!(
                matches!(
                    k,
                    FailureKind::Busy
                        | FailureKind::Limit
                        | FailureKind::Trap
                        | FailureKind::Malformed
                        | FailureKind::ModuleFailed
                        | FailureKind::Cancelled
                ),
                "an untyped failure: {k:?}"
            );
            seen.insert(format!("{k:?}"));
        }
    }
    assert!(
        seen.contains("Busy"),
        "the pool was small enough to refuse: {seen:?}"
    );
    assert!(seen.contains("Limit") && seen.contains("Trap"), "{seen:?}");
    assert!(
        pool.peak.load(Ordering::SeqCst) <= budget,
        "the peak never passes the pool"
    );
    assert_eq!(
        pool.held.load(Ordering::SeqCst),
        code,
        "every call released what it held"
    );
    // The node still serves.
    let ok = host
        .load(
            &build(&Parts::default()),
            Arc::new(Handle(Arc::clone(&pool))),
        )
        .expect("loads");
    assert_eq!(ok.call("t", 0, b"x").expect("served"), b"x");
}

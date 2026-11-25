//! Reservations and the concurrency gate (docs/specs/extension-abi AC-6).

mod common;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Handle, IO, Parts, Pool, build, descriptor, with_body};
use oss_ext::FailureKind;
use oss_ext_wasm::{Host, HostConfig, LoadError, Module, Unbounded};

fn load(host: &Host, wasm: &[u8]) -> Module {
    host.load(wasm, Arc::new(Unbounded)).expect("loads")
}

fn host() -> Host {
    Host::new(HostConfig::default())
}

// docs/specs/extension-abi AC-6
#[test]
fn a_call_holds_exactly_the_reservation_formula_and_releases_it() {
    let seen = Arc::new(Mutex::new(Vec::<u64>::new()));
    let pool = Pool::of(u64::MAX);
    let (p, sink) = (Arc::clone(&pool), Arc::clone(&seen));
    let host = Host::new(HostConfig {
        log: Arc::new(move |_, _| sink.lock().unwrap().push(p.held.load(Ordering::SeqCst))),
        ..HostConfig::default()
    });
    let wasm = with_body("(call $log (i32.const 1000) (i32.const 4)) (i32.const 0)");
    let m = host
        .load(&wasm, Arc::new(Handle(Arc::clone(&pool))))
        .expect("loads");
    let code = u64::try_from(wasm.len()).unwrap() * 16;
    assert_eq!(
        pool.held.load(Ordering::SeqCst),
        code,
        "compiled code is charged while loaded"
    );
    let formula = 64 * 1024 * 1024 + 2 * u64::from(IO) + 1024 * 1024;
    assert_eq!(m.call_reservation(), formula);
    m.call("t", 0, b"").expect("ok");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![code + formula],
        "held during the call"
    );
    assert_eq!(pool.held.load(Ordering::SeqCst), code, "released after it");
    drop(m);
    assert_eq!(pool.held.load(Ordering::SeqCst), 0, "and the code on drop");
}

#[test]
fn a_pool_that_cannot_hold_a_call_answers_busy() {
    let wasm = with_body("(i32.const 0)");
    let code = u64::try_from(wasm.len()).unwrap() * 16;
    let pool = Pool::of(code + 65 * 1024 * 1024);
    let m = host()
        .load(&wasm, Arc::new(Handle(Arc::clone(&pool))))
        .expect("loads");
    let e = m.call("t", 0, b"").unwrap_err();
    assert_eq!((e.kind(), e.http_status()), (FailureKind::Busy, 429));
    let none = Pool::of(code - 1);
    let e = host()
        .load(&wasm, Arc::new(Handle(none)))
        .err()
        .expect("refused");
    assert!(matches!(e, LoadError::Busy));
}

#[test]
fn a_tenant_is_held_to_its_share_and_others_are_still_served() {
    let host = Host::new(HostConfig {
        fuel: u64::MAX / 2,
        deadline_ms: 600,
        ..HostConfig::default()
    });
    let m = Arc::new(load(&host, &with_body("(loop $l (br $l)) (i32.const 0)")));
    let spinners: Vec<_> = (0..2)
        .map(|_| {
            let m = Arc::clone(&m);
            std::thread::spawn(move || m.call("a", 0, b"").map(drop))
        })
        .collect();
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(m.call("a", 0, b"").unwrap_err().kind(), FailureKind::Busy);
    let other = m.call("b", 0, b"").unwrap_err();
    assert_ne!(
        other.kind(),
        FailureKind::Busy,
        "another tenant has its own share"
    );
    for s in spinners {
        assert!(s.join().unwrap().is_err());
    }
    // The gate is open again.
    assert_ne!(m.call("a", 0, b"").unwrap_err().kind(), FailureKind::Busy);
}

#[test]
fn the_module_cap_holds_below_the_host_default() {
    let one = Parts {
        descriptor: descriptor("")
            .replace(r#""max_concurrent_calls":8"#, r#""max_concurrent_calls":1"#),
        body: "(loop $l (br $l)) (i32.const 0)",
        ..Parts::default()
    };
    let host = Host::new(HostConfig {
        fuel: u64::MAX / 2,
        deadline_ms: 500,
        ..HostConfig::default()
    });
    let m = Arc::new(load(&host, &build(&one)));
    let m2 = Arc::clone(&m);
    let t = std::thread::spawn(move || m2.call("a", 0, b"").map(drop));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(m.call("b", 0, b"").unwrap_err().kind(), FailureKind::Busy);
    assert!(t.join().unwrap().is_err());
}

#[test]
fn describing_is_reserved_too_and_a_refused_call_frees_its_place() {
    let wasm = with_body("(i32.const 0)");
    let code = u64::try_from(wasm.len()).unwrap() * 16;
    // Room for the compile charge only: describing cannot be held.
    let e = host()
        .load(&wasm, Arc::new(Handle(Pool::of(code))))
        .err()
        .expect("busy");
    assert!(matches!(e, LoadError::Busy));
    // A call the pool refused leaves the tenant's place free.
    let pool = Pool::of(code + 65 * 1024 * 1024 + 1024 * 1024);
    let host = Host::new(HostConfig {
        tenant_concurrency: 1,
        ..HostConfig::default()
    });
    let m = host
        .load(&wasm, Arc::new(Handle(Arc::clone(&pool))))
        .expect("loads");
    let hog = Handle(Arc::clone(&pool));
    let held = oss_ext_wasm::Reserver::reserve(&hog, u64::MAX / 4);
    assert!(held.is_none());
    pool.budget.store(code, Ordering::SeqCst);
    assert_eq!(m.call("t", 0, b"").unwrap_err().kind(), FailureKind::Busy);
    pool.budget.store(u64::MAX, Ordering::SeqCst);
    assert!(
        m.call("t", 0, b"").is_ok(),
        "the refused call released its place"
    );
}

//! Hostile and malformed modules (docs/specs/extension-abi AC-5, AC-6): each
//! fails its call or its load with a typed failure, and the host keeps serving.

mod common;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Handle, IO, Parts, Pool, build, descriptor, with_body};
use oss_ext::{FailureKind, VersionError};
use oss_ext_wasm::{Host, HostConfig, LoadError, Module, Unbounded};

fn load(host: &Host, wasm: &[u8]) -> Module {
    host.load(wasm, Arc::new(Unbounded)).expect("loads")
}

fn host() -> Host {
    Host::new(HostConfig::default())
}

fn kind(m: &Module) -> (FailureKind, u16, String) {
    let e = m.call("t", 0, b"hi").expect_err("the call fails");
    (e.kind(), e.http_status(), e.detail().to_owned())
}

// docs/specs/extension-abi AC-5
#[test]
fn a_well_behaved_module_echoes_and_describes_itself() {
    let m = load(&host(), &build(&Parts::default()));
    assert_eq!(m.descriptor().name(), "hostile");
    assert_eq!(m.call("t", 0, b"hello").expect("echo"), b"hello");
    assert_eq!(m.call("t", 0, b"").expect("empty"), b"");
    // A second call sees a fresh instance, not the first call's state.
    assert_eq!(m.call("t", 0, b"again").expect("echo"), b"again");
}

#[test]
fn a_point_that_is_not_in_the_descriptor_and_a_large_input_are_refused() {
    let m = load(&host(), &build(&Parts::default()));
    assert_eq!(
        m.call("t", 1, b"x").unwrap_err().kind(),
        FailureKind::Unsupported
    );
    let big = vec![0_u8; IO as usize + 1];
    let e = m.call("t", 0, &big).unwrap_err();
    assert_eq!((e.kind(), e.http_status()), (FailureKind::Limit, 400));
    assert_eq!(
        m.call("t", 0, &big[..IO as usize])
            .expect("exactly the region")
            .len(),
        IO as usize
    );
}

#[test]
fn a_spinning_module_runs_out_of_fuel_and_the_next_call_is_served() {
    let host = host();
    let spin = load(&host, &with_body("(loop $l (br $l)) (i32.const 0)"));
    assert_eq!(
        kind(&spin),
        (FailureKind::Limit, 400, "fuel exhausted".into())
    );
    let ok = load(&host, &build(&Parts::default()));
    assert!(ok.call("t", 0, b"x").is_ok());
}

#[test]
fn a_module_that_outlasts_its_deadline_is_interrupted() {
    let host = Host::new(HostConfig {
        fuel: u64::MAX / 2,
        deadline_ms: 100,
        ..HostConfig::default()
    });
    let spin = load(&host, &with_body("(loop $l (br $l)) (i32.const 0)"));
    let started = std::time::Instant::now();
    assert_eq!(
        kind(&spin),
        (FailureKind::Limit, 400, "deadline passed".into())
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_cooperative_module_is_told_to_stop_before_the_hard_deadline() {
    let host = Host::new(HostConfig {
        fuel: u64::MAX / 2,
        deadline_ms: 400,
        ..HostConfig::default()
    });
    let m = load(
        &host,
        &with_body("(loop $l (br_if $l (i32.eqz (call $cancelled)))) (i32.const -5)"),
    );
    assert_eq!(
        m.call("t", 0, b"").unwrap_err().kind(),
        FailureKind::Cancelled
    );
}

#[test]
fn growing_memory_past_the_cap_fails_the_growth_and_not_the_node() {
    let grow = "(if (i32.eq (memory.grow (i32.const 100000)) (i32.const -1)) \
        (then (return (i32.const -4)))) (i32.const 0)";
    let m = load(&host(), &with_body(grow));
    assert_eq!(m.call("t", 0, b"").unwrap_err().kind(), FailureKind::Limit);
    // Below the cap the same growth succeeds.
    let small = "(if (i32.eq (memory.grow (i32.const 10)) (i32.const -1)) \
        (then (return (i32.const -4)))) (i32.const 0)";
    assert!(load(&host(), &with_body(small)).call("t", 0, b"").is_ok());
}

#[test]
fn a_descriptor_may_lower_the_memory_cap_and_cannot_raise_it() {
    let lower = Parts {
        descriptor: descriptor(r#","limits":{"memory_bytes":1048576}"#),
        body: "(if (i32.eq (memory.grow (i32.const 100)) (i32.const -1)) \
            (then (return (i32.const -4)))) (i32.const 0)",
        ..Parts::default()
    };
    assert_eq!(
        load(&host(), &build(&lower))
            .call("t", 0, b"")
            .unwrap_err()
            .kind(),
        FailureKind::Limit
    );
    let raise = Parts {
        descriptor: descriptor(r#","limits":{"memory_bytes":4294967296}"#),
        body: "(if (i32.eq (memory.grow (i32.const 100000)) (i32.const -1)) \
            (then (return (i32.const -4)))) (i32.const 0)",
        ..Parts::default()
    };
    assert_eq!(
        load(&host(), &build(&raise))
            .call("t", 0, b"")
            .unwrap_err()
            .kind(),
        FailureKind::Limit
    );
}

#[test]
fn a_module_whose_memory_starts_over_the_cap_is_not_loaded() {
    let p = Parts {
        memory: "2000",
        ..Parts::default()
    };
    let e = host()
        .load(&build(&p), Arc::new(Unbounded))
        .err()
        .expect("refused");
    assert!(matches!(e, LoadError::Broken(_)), "{e}");
}

#[test]
fn a_trap_is_a_trap_and_a_runaway_recursion_is_a_stack_limit() {
    let host = host();
    assert_eq!(
        kind(&load(&host, &with_body("(unreachable)"))).0,
        FailureKind::Trap
    );
    let rec = Parts {
        body: "(call $rec) (i32.const 0)",
        extra: "(func $rec (call $rec))",
        ..Parts::default()
    };
    assert_eq!(
        kind(&load(&host, &build(&rec))),
        (FailureKind::Limit, 400, "stack exhausted".into())
    );
}

#[test]
fn an_output_past_its_region_or_memory_is_malformed() {
    let host = host();
    for len in [IO + 1, u32::MAX] {
        let body = format!("(global.set $len (i32.const {})) (i32.const 0)", len as i32);
        let (k, status, _) = kind(&load(&host, &with_body(&body)));
        assert_eq!((k, status), (FailureKind::Malformed, 502), "length {len}");
    }
}

#[test]
fn an_undefined_status_is_malformed() {
    let (k, status, detail) = kind(&load(&host(), &with_body("(i32.const -99)")));
    assert_eq!((k, status), (FailureKind::Malformed, 502));
    assert_eq!(detail, "undefined status -99");
}

#[test]
fn a_module_failure_carries_its_detail_made_printable() {
    let body = "(memory.copy (i32.const 131072) (i32.const 400) (i32.const 8)) \
        (global.set $len (i32.const 8)) (i32.const -6)";
    let (k, status, detail) = kind(&load(&host(), &with_body(body)));
    assert_eq!((k, status), (FailureKind::ModuleFailed, 400));
    assert_eq!(detail, "bad?news");
}

#[test]
fn a_module_that_asks_for_more_than_the_output_holds_fails_without_a_retry() {
    let (k, _, _) = kind(&load(&host(), &with_body("(i32.const -1)")));
    assert_eq!(k, FailureKind::OutputTooLarge);
}

#[test]
fn log_lines_are_cut_rate_limited_and_made_printable() {
    let seen = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let sink = Arc::clone(&seen);
    let host = Host::new(HostConfig {
        log: Arc::new(move |name, text| {
            sink.lock()
                .unwrap()
                .push((name.to_owned(), text.to_owned()));
        }),
        ..HostConfig::default()
    });
    let body = "(local $i i32) (loop $l (call $log (i32.const 1000) (i32.const 600)) \
        (local.set $i (i32.add (local.get $i) (i32.const 1))) \
        (br_if $l (i32.lt_u (local.get $i) (i32.const 20)))) (i32.const 0)";
    load(&host, &with_body(body)).call("t", 0, b"").expect("ok");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 10, "ten a second");
    assert!(
        seen.iter()
            .all(|(n, t)| n == "hostile" && t == &"?".repeat(512))
    );
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
    let pool = Pool::of(code + 1);
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

// Loading: what is refused before or after the compile.
#[test]
fn a_module_that_is_refused_names_why() {
    let host = host();
    let refused = |p: &Parts<'_>, want: &str| {
        let e = host
            .load(&build(p), Arc::new(Unbounded))
            .err()
            .expect("refused")
            .to_string();
        assert!(e.contains(want), "{e} should contain {want}");
    };
    refused(
        &Parts {
            imports: r#"(import "env" "x" (func))"#,
            ..Parts::default()
        },
        "env.x",
    );
    refused(
        &Parts {
            extra: "(func $s) (start $s)",
            ..Parts::default()
        },
        "start function",
    );
    refused(
        &Parts {
            extra: "(memory 1)",
            ..Parts::default()
        },
        "more than one memory",
    );
    let locals = format!("(func (local {}))", "i32 ".repeat(1001));
    refused(
        &Parts {
            extra: &locals,
            ..Parts::default()
        },
        "locals",
    );
    let many = "(func)".repeat(5001);
    refused(
        &Parts {
            extra: &many,
            ..Parts::default()
        },
        "functions",
    );
    let e = host
        .load(b"not wasm", Arc::new(Unbounded))
        .err()
        .expect("refused");
    assert!(matches!(e, LoadError::Refused(_)));
    let huge = vec![0_u8; 8 * 1024 * 1024 + 1];
    assert!(matches!(
        host.load(&huge, Arc::new(Unbounded)).err(),
        Some(LoadError::TooLarge)
    ));
}

#[test]
fn a_module_with_the_wrong_abi_or_descriptor_or_exports_is_not_loaded() {
    let host = host();
    let e = host
        .load(
            &build(&Parts {
                abi: 2 << 16,
                ..Parts::default()
            }),
            Arc::new(Unbounded),
        )
        .err()
        .expect("refused");
    assert!(matches!(
        e,
        LoadError::Abi(VersionError::OtherMajor { module: 2, host: 1 })
    ));
    let e = host
        .load(
            &build(&Parts {
                abi: (1 << 16) | 1,
                ..Parts::default()
            }),
            Arc::new(Unbounded),
        )
        .err()
        .expect("refused");
    assert!(matches!(e, LoadError::Abi(VersionError::NewerMinor { .. })));
    let e = host
        .load(
            &build(&Parts {
                descriptor: descriptor(r#","surprise":1"#),
                ..Parts::default()
            }),
            Arc::new(Unbounded),
        )
        .err()
        .expect("refused");
    assert!(
        matches!(&e, LoadError::Descriptor(d) if d.key() == "surprise"),
        "{e}"
    );
    let no_describe = wat::parse_str(
        r#"(module (memory (export "memory") 1) (func (export "oss_abi_version") (result i32) (i32.const 65536)))"#,
    )
    .unwrap();
    let e = host
        .load(&no_describe, Arc::new(Unbounded))
        .err()
        .expect("refused");
    assert!(matches!(e, LoadError::Broken(_)), "{e}");
}

#[test]
fn regions_outside_memory_or_overlapping_are_refused() {
    let host = host();
    // Memory of one page puts both regions out of range.
    let e = host
        .load(
            &build(&Parts {
                memory: "1",
                ..Parts::default()
            }),
            Arc::new(Unbounded),
        )
        .err()
        .expect("refused");
    assert!(matches!(e, LoadError::Broken(_)), "{e}");
}

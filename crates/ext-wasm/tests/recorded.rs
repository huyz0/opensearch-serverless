//! A call under limits a descriptor recorded, and a replayed one
//! ([ADR 0053](../../../docs/adr/0053-the-analyzer-extension-point.md)):
//! the recorded numbers are the limit, not the host's present ones, and a
//! replay is never refused for capacity.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Handle, Pool, with_body};
use oss_ext::FailureKind;
use oss_ext::pin::ResolvedLimits;
use oss_ext_wasm::{Host, HostConfig, Module, Recovery, Replay, Run, Unbounded};

/// A module that does a fixed amount of work: a loop of 200,000 turns.
const WORK: &str = "(local $i i32) (loop $l (local.set $i (i32.add (local.get $i) (i32.const 1))) \
    (br_if $l (i32.lt_u (local.get $i) (i32.const 200000)))) (i32.const 0)";

fn load(host: &Host, body: &str) -> Module {
    host.load(&with_body(body), Arc::new(Unbounded))
        .expect("loads")
}

fn limits(fuel: u64) -> ResolvedLimits {
    ResolvedLimits {
        fuel,
        memory_bytes: 64 * 1024 * 1024,
        deadline_ms: 10_000,
    }
}

fn kind(m: &Module, run: Run) -> Option<FailureKind> {
    m.call_as("t", 0, b"", run).err().map(|e| e.kind())
}

#[test]
fn a_recorded_fuel_is_the_limit_whatever_the_host_allows_now() {
    // The host allows little now; the descriptor recorded more.
    let host = Host::new(HostConfig {
        fuel: 1_000,
        ..HostConfig::default()
    });
    let m = load(&host, WORK);
    assert_eq!(m.call("t", 0, b"").unwrap_err().kind(), FailureKind::Limit);
    assert_eq!(kind(&m, Run::Recorded(limits(100_000_000))), None);
    // And a host that allows a lot does not raise a small record.
    let host = Host::new(HostConfig {
        fuel: 100_000_000,
        ..HostConfig::default()
    });
    let m = load(&host, WORK);
    assert!(m.call("t", 0, b"").is_ok());
    assert_eq!(
        kind(&m, Run::Recorded(limits(10_000))),
        Some(FailureKind::Limit)
    );
}

#[test]
fn a_replay_runs_under_the_record_times_its_factor() {
    let m = load(&Host::new(HostConfig::default()), WORK);
    let replay = |factor| {
        Run::Replay(Replay {
            limits: limits(60_000),
            factor,
            wait: Duration::ZERO,
        })
    };
    assert_eq!(kind(&m, replay(1)), Some(FailureKind::Limit));
    assert_eq!(kind(&m, replay(1_000)), None);
}

#[test]
fn a_replay_is_not_refused_by_a_full_gate() {
    let host = Host::new(HostConfig {
        fuel: u64::MAX / 2,
        deadline_ms: 700,
        ..HostConfig::default()
    });
    let m = Arc::new(load(&host, "(loop $l (br $l)) (i32.const 0)"));
    let spinners: Vec<_> = (0..2)
        .map(|_| {
            let m = Arc::clone(&m);
            std::thread::spawn(move || m.call("a", 0, b"").map(drop))
        })
        .collect();
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(m.call("a", 0, b"").unwrap_err().kind(), FailureKind::Busy);
    let run = Run::Replay(Replay {
        limits: limits(1_000),
        factor: 1,
        wait: Duration::ZERO,
    });
    // The tenant's share is spent, and the replay is let through to fail on
    // its own fuel instead.
    assert_eq!(kind(&m, run), Some(FailureKind::Limit));
    for s in spinners {
        assert!(s.join().unwrap().is_err());
    }
}

#[test]
fn a_replay_waits_for_its_reservation_and_then_answers_busy() {
    let wasm = with_body("(i32.const 0)");
    let code = u64::try_from(wasm.len()).unwrap() * 16;
    let pool = Pool::of(code + 65 * 1024 * 1024);
    let m = Host::new(HostConfig::default())
        .load(&wasm, Arc::new(Handle(Arc::clone(&pool))))
        .expect("loads");
    let run = |wait| {
        Run::Replay(Replay {
            limits: limits(1_000_000),
            factor: 1,
            wait: Duration::from_millis(wait),
        })
    };
    // The pool cannot hold a call: a replay waits its time, then is busy.
    let started = Instant::now();
    assert_eq!(kind(&m, run(300)), Some(FailureKind::Busy));
    assert!(started.elapsed() >= Duration::from_millis(300));
    // A call that is refused at once is not a replay.
    let started = Instant::now();
    assert_eq!(m.call("t", 0, b"").unwrap_err().kind(), FailureKind::Busy);
    assert!(started.elapsed() < Duration::from_millis(250));
}

#[test]
fn a_recovery_runs_under_the_record_exactly_and_is_not_refused_by_a_full_gate() {
    let host = Host::new(HostConfig {
        fuel: u64::MAX / 2,
        deadline_ms: 700,
        ..HostConfig::default()
    });
    let m = Arc::new(load(&host, "(loop $l (br $l)) (i32.const 0)"));
    let spinners: Vec<_> = (0..2)
        .map(|_| {
            let m = Arc::clone(&m);
            std::thread::spawn(move || m.call("a", 0, b"").map(drop))
        })
        .collect();
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(m.call("a", 0, b"").unwrap_err().kind(), FailureKind::Busy);
    let run = Run::Recovery(Recovery {
        limits: limits(1_000),
        wait: Duration::ZERO,
    });
    // Let through the full share, it fails on the recorded fuel, not on the gate.
    let e = m.call_as("a", 0, b"", run).unwrap_err();
    assert_eq!(e.kind(), FailureKind::Limit);
    for s in spinners {
        assert!(s.join().unwrap().is_err());
    }
}

#[test]
fn a_recovery_is_held_to_the_record_without_the_replay_stretch() {
    let m = load(&Host::new(HostConfig::default()), WORK);
    let run = |fuel| {
        Run::Recovery(Recovery {
            limits: limits(fuel),
            wait: Duration::ZERO,
        })
    };
    assert_eq!(kind(&m, run(60_000)), Some(FailureKind::Limit));
    assert_eq!(kind(&m, run(100_000_000)), None);
}

#[test]
fn a_recovery_waits_for_its_reservation_and_then_answers_busy() {
    let wasm = with_body("(i32.const 0)");
    let code = u64::try_from(wasm.len()).unwrap() * 16;
    let pool = Pool::of(code + 65 * 1024 * 1024);
    let m = Host::new(HostConfig::default())
        .load(&wasm, Arc::new(Handle(Arc::clone(&pool))))
        .expect("loads");
    let started = Instant::now();
    let run = Run::Recovery(Recovery {
        limits: limits(1_000_000),
        wait: Duration::from_millis(300),
    });
    assert_eq!(kind(&m, run), Some(FailureKind::Busy));
    assert!(started.elapsed() >= Duration::from_millis(300));
}

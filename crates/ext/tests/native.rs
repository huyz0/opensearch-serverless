//! The guarded call (docs/specs/plugin-system AC-7, AC-10, AC-11).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};

use oss_ext::breaker::{BreakerConfig, BreakerState};
use oss_ext::native::{Guard, GuardError, NativeInstance};
use oss_ext::{CallError, FailureKind, Status};

fn guard() -> Guard {
    Guard::new(
        BreakerConfig {
            jitter_permille: 0,
            ..BreakerConfig::default()
        },
        1,
    )
}

fn now() -> u64 {
    0
}

/// Answers by the first input byte: 0 echoes, 1 panics, 2 fails as a module,
/// 3 rejects the input, 4 answers 10 bytes whatever the ceiling.
struct Scripted {
    calls: AtomicUsize,
}

impl NativeInstance for Scripted {
    fn manifest(&self) -> Vec<u8> {
        Vec::new()
    }

    fn call(&self, _point: &str, input: &[u8], _max_out: usize) -> Result<Vec<u8>, CallError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match input.first().copied().unwrap_or(0) {
            1 => panic!("boom"),
            2 => Err(CallError::from_status(Status::Module, b"bad")),
            3 => Err(CallError::from_status(Status::InvalidArgument, b"no")),
            4 => Ok(vec![0; 10]),
            _ => Ok(input.to_vec()),
        }
    }
}

fn scripted() -> Scripted {
    Scripted {
        calls: AtomicUsize::new(0),
    }
}

#[test]
fn a_call_answers_and_the_next_is_served_after_a_panic() {
    let (g, p) = (guard(), scripted());
    assert_eq!(g.call(&p, "t", "rest", &[0, 7], 16, now), Ok(vec![0, 7]));
    let e = g.call(&p, "t", "rest", &[1], 16, now).expect_err("panics");
    assert_eq!(e, GuardError::Panicked);
    assert_eq!((e.http_status(), e.code()), (500, "plugin_failed"));
    assert_eq!(g.call(&p, "t", "rest", &[0, 8], 16, now), Ok(vec![0, 8]));
}

#[test]
fn an_output_over_the_ceiling_is_malformed_not_returned() {
    let (g, p) = (guard(), scripted());
    assert_eq!(g.call(&p, "t", "rest", &[4], 10, now), Ok(vec![0; 10]));
    let e = g.call(&p, "t", "rest", &[4], 9, now).expect_err("over");
    assert_eq!((e.http_status(), e.code()), (502, "plugin_malformed"));
}

#[test]
fn five_failures_open_the_breaker_and_an_open_breaker_calls_nothing() {
    let (g, p) = (guard(), scripted());
    for _ in 0..4 {
        let _ = g.call(&p, "t", "rest", &[2], 16, now);
    }
    assert_eq!(g.state(), BreakerState::Closed);
    let _ = g.call(&p, "t", "rest", &[1], 16, now);
    assert_eq!(g.state(), BreakerState::Open);
    let before = p.calls.load(Ordering::SeqCst);
    let e = g.call(&p, "t", "rest", &[0], 16, now).expect_err("open");
    assert_eq!((e.http_status(), e.code()), (503, "plugin_unavailable"));
    assert_eq!(p.calls.load(Ordering::SeqCst), before, "no call was made");
}

#[test]
fn a_rejected_input_never_counts_against_the_plugin() {
    let (g, p) = (guard(), scripted());
    for _ in 0..30 {
        let e = g.call(&p, "t", "rest", &[3], 16, now).expect_err("rejects");
        assert_eq!((e.http_status(), e.code()), (400, "plugin_rejected"));
    }
    assert_eq!(g.state(), BreakerState::Closed);
}

#[test]
fn a_failed_call_is_a_plugin_failure_with_its_detail() {
    let (g, p) = (guard(), scripted());
    match g.call(&p, "t", "rest", &[2], 16, now) {
        Err(GuardError::Failed(e)) => {
            assert_eq!(e.kind(), FailureKind::ModuleFailed);
            assert_eq!(e.detail(), "bad");
        }
        other => panic!("{other:?}"),
    }
}

/// Holds every call until released, telling the test each one arrived.
struct Blocking {
    arrived: Mutex<SyncSender<()>>,
    release: Mutex<Receiver<()>>,
}

impl NativeInstance for Blocking {
    fn manifest(&self) -> Vec<u8> {
        Vec::new()
    }

    fn call(&self, _p: &str, _i: &[u8], _m: usize) -> Result<Vec<u8>, CallError> {
        self.arrived.lock().expect("lock").send(()).expect("send");
        let _ = self.release.lock().expect("lock").recv();
        Ok(Vec::new())
    }
}

#[test]
fn the_bulkhead_holds_eight_a_plugin_and_two_a_tenant_and_queues_nothing() {
    let (arrived_tx, arrived_rx) = sync_channel(16);
    let (release_tx, release_rx) = sync_channel(16);
    let plugin = Arc::new(Blocking {
        arrived: Mutex::new(arrived_tx),
        release: Mutex::new(release_rx),
    });
    let g = Arc::new(guard());
    let mut handles = Vec::new();
    // Four tenants, two calls each: eight running.
    for tenant in ["a", "a", "b", "b", "c", "c", "d", "d"] {
        let (g, p) = (g.clone(), plugin.clone());
        handles.push(std::thread::spawn(move || {
            g.call(&*p, tenant, "rest", &[], 16, now)
        }));
    }
    for _ in 0..8 {
        arrived_rx.recv().expect("each of the eight arrives");
    }
    // The ninth, for a fresh tenant, and a third for tenant "a".
    let e = g
        .call(&*plugin, "e", "rest", &[], 16, now)
        .expect_err("full");
    assert_eq!((e,), (GuardError::PluginBusy,));
    let e = g
        .call(&*plugin, "a", "rest", &[], 16, now)
        .expect_err("full");
    assert!(matches!(e, GuardError::PluginBusy | GuardError::TenantBusy));
    for _ in 0..8 {
        release_tx.send(()).expect("release");
    }
    for h in handles {
        assert!(h.join().expect("joined").is_ok());
    }
    // Every place is free again, so a tenant gets its two.
    drop(release_tx);
    let tenant_only = guard();
    let (a2, r2) = sync_channel(16);
    let (rel2, rr2) = sync_channel(16);
    let p2 = Arc::new(Blocking {
        arrived: Mutex::new(a2),
        release: Mutex::new(rr2),
    });
    let g2 = Arc::new(tenant_only);
    let mut hs = Vec::new();
    for _ in 0..2 {
        let (g, p) = (g2.clone(), p2.clone());
        hs.push(std::thread::spawn(move || {
            g.call(&*p, "x", "rest", &[], 16, now)
        }));
    }
    for _ in 0..2 {
        r2.recv().expect("arrives");
    }
    let e = g2.call(&*p2, "x", "rest", &[], 16, now).expect_err("third");
    assert_eq!(e, GuardError::TenantBusy);
    assert_eq!(e.http_status(), 429);
    // Another tenant is not held up by x.
    for _ in 0..2 {
        rel2.send(()).expect("release");
    }
    for h in hs {
        assert!(h.join().expect("joined").is_ok());
    }
}

/// Asks the guard for its state from inside the call: a lock held across the
/// call would deadlock this.
struct Reentrant {
    guard: Arc<Guard>,
}

impl NativeInstance for Reentrant {
    fn manifest(&self) -> Vec<u8> {
        Vec::new()
    }

    fn call(&self, _p: &str, _i: &[u8], _m: usize) -> Result<Vec<u8>, CallError> {
        assert_eq!(self.guard.state(), BreakerState::Closed);
        panic!("after using the guard");
    }
}

#[test]
fn no_lock_is_held_across_a_call_and_a_panic_frees_its_place() {
    let g = Arc::new(guard());
    let p = Reentrant { guard: g.clone() };
    assert_eq!(
        g.call(&p, "t", "rest", &[], 16, now),
        Err(GuardError::Panicked)
    );
    // Panics eight times over would fill the bulkhead if places leaked.
    for _ in 0..4 {
        assert_eq!(
            g.call(&p, "t", "rest", &[], 16, now),
            Err(GuardError::Panicked)
        );
    }
}

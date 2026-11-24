//! docs/specs/search-slots: the gate alone.

// ARITH: test counts and durations are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;
use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};

use oss_node::search_slots::{Config, SearchSlots};

fn gate(slots: usize, share_pct: usize, wait_ms: u64, waiter_cap: usize) -> Arc<SearchSlots> {
    Arc::new(SearchSlots::new(&Config {
        slots,
        fair_share_pct: share_pct,
        wait_millis: wait_ms,
        waiter_cap,
    }))
}

/// Takes `n` permits for `tenant` and keeps them.
fn hold<'a>(
    g: &'a SearchSlots,
    tenant: &str,
    n: usize,
) -> Vec<oss_node::search_slots::Admission<'a>> {
    (0..n).map(|_| g.admit(tenant, 0)).collect()
}

// AC-1
#[test]
fn a_permit_is_taken_and_given_back_and_the_tenant_entry_goes() {
    let g = gate(4, 25, 100, 2);
    {
        let a = g.admit("a", 0);
        assert!(a.holds_permit());
        let r = g.report();
        assert_eq!((r.in_use, r.tenants), (1, 1));
    }
    let r = g.report();
    assert_eq!((r.in_use, r.tenants), (0, 0));
}

// AC-2
#[test]
fn a_gate_that_is_not_full_admits_at_once_and_a_full_one_lets_a_tenant_within_its_share_run() {
    let g = gate(8, 25, 1_000, 4);
    let started = Instant::now();
    let a = hold(&g, "a", 6);
    let b = hold(&g, "b", 2);
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(g.report().in_use, 8);
    // Full. `c` holds nothing: within its share, runs at once without a permit.
    let c = g.admit("c", 0);
    assert!(!c.holds_permit());
    assert!(started.elapsed() < Duration::from_millis(500));
    let r = g.report();
    assert_eq!((r.in_use, r.overflow_full, r.waited), (8, 1, 0));
    drop((a, b, c));
}

// AC-3
#[test]
fn a_tenant_over_its_share_waits_on_a_full_gate_and_a_quiet_one_does_not() {
    let g = gate(8, 25, 800, 4);
    let a = hold(&g, "a", 6);
    let b = hold(&g, "b", 2);
    let (tx, rx) = sync_channel(1);
    let waiter = {
        let g = Arc::clone(&g);
        std::thread::spawn(move || {
            let started = Instant::now();
            let adm = g.admit("a", 0);
            let _ = tx.send((started.elapsed(), adm.holds_permit()));
        })
    };
    while g.report().waiting == 0 {
        std::thread::sleep(Duration::from_millis(5));
    }
    let quiet = Instant::now();
    let c = g.admit("c", 0);
    assert!(quiet.elapsed() < Duration::from_millis(300), "c waited");
    drop(c);
    drop(b);
    let (waited, permit) = rx.recv_timeout(Duration::from_secs(5)).expect("a ran");
    // AC-7 as well: b's permits freed the gate, so a got one before its deadline.
    assert!(
        permit && waited < Duration::from_millis(800),
        "{waited:?} {permit}"
    );
    waiter.join().expect("joins");
    drop(a);
}

// AC-4
#[test]
fn a_wait_ends_at_its_deadline_and_the_search_runs_as_an_overflow_runner() {
    let g = gate(8, 25, 150, 4);
    let _a = hold(&g, "a", 6);
    let _b = hold(&g, "b", 2);
    let started = Instant::now();
    let adm = g.admit("a", 0);
    let waited = started.elapsed();
    assert!(!adm.holds_permit());
    assert!(
        waited >= Duration::from_millis(150) && waited < Duration::from_secs(2),
        "{waited:?}"
    );
    let r = g.report();
    assert_eq!((r.waited, r.past_wait, r.waiting), (1, 1, 0));
    assert!(r.longest_wait_millis >= 150);
}

// AC-5
#[test]
fn past_the_waiter_cap_an_over_share_search_runs_at_once() {
    let g = gate(8, 25, 2_000, 1);
    let _a = hold(&g, "a", 6);
    let _b = hold(&g, "b", 2);
    let first = {
        let g = Arc::clone(&g);
        std::thread::spawn(move || drop(g.admit("a", 0)))
    };
    while g.report().waiting == 0 {
        std::thread::sleep(Duration::from_millis(5));
    }
    let started = Instant::now();
    let adm = g.admit("a", 0);
    assert!(!adm.holds_permit());
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(g.report().overflow_cap, 1);
    drop(adm);
    drop(_b);
    drop(_a);
    first.join().expect("joins");
}

// AC-6
#[test]
fn a_level_two_tenant_waits_at_half_the_slots_only_with_another_tenant_active() {
    let g = gate(8, 100, 200, 4);
    // Alone at level 3, past half the slots: never waits on itself.
    let alone = hold(&g, "a", 5);
    let started = Instant::now();
    let held = g.admit("a", 3);
    assert!(held.holds_permit() && started.elapsed() < Duration::from_millis(150));
    drop(held);
    drop(alone);
    // With b holding a permit and a past half the slots: level 2 waits, level 0 does not.
    let _b = hold(&g, "b", 1);
    let _a = hold(&g, "a", 4);
    let t = Instant::now();
    let zero = g.admit("a", 0);
    assert!(
        zero.holds_permit() && t.elapsed() < Duration::from_millis(150),
        "level 0 waited"
    );
    drop(zero);
    let t = Instant::now();
    let two = g.admit("a", 2);
    assert!(
        !two.holds_permit(),
        "a level-2 tenant got a permit past half"
    );
    assert!(
        t.elapsed() >= Duration::from_millis(200),
        "{:?}",
        t.elapsed()
    );
}

// AC-8
#[test]
fn the_report_counts_what_happened() {
    let g = gate(2, 100, 50, 2);
    let _a = hold(&g, "a", 2);
    drop(g.admit("b", 0));
    let r = g.report();
    assert_eq!((r.slots, r.in_use, r.overflow_full), (2, 2, 1));
}

// AC-9
#[test]
fn the_state_holds_one_entry_a_tenant_and_no_more() {
    let g = gate(6, 100, 3_000, 2);
    let names = ["a", "b", "c", "d", "e", "f"];
    let held: Vec<_> = names.iter().map(|n| g.admit(n, 0)).collect();
    assert_eq!(g.report().tenants, 6);
    // Waiters at the cap: tenants g and h wait, the third runs at once.
    let waiters: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|n| {
            let g = Arc::clone(&g);
            std::thread::spawn(move || drop(g.admit(n, 3)))
        })
        .collect();
    while g.report().waiting < 2 {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(g.report().tenants <= 6);
    let extra = g.admit("a", 3);
    assert!(!extra.holds_permit());
    drop(extra);
    drop(held);
    for w in waiters {
        w.join().expect("joins");
    }
    assert_eq!(g.report().tenants, 0);
}

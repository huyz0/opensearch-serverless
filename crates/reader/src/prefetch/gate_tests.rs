//! The gate's limits.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::Gate;

#[test]
fn a_grant_past_either_limit_is_refused_and_leaves_nothing_taken() {
    let gate = Gate::new(100, 2);
    let a = gate.try_take(60).expect("fits");
    assert!(gate.try_take(60).is_none(), "bytes would pass the limit");
    assert_eq!(gate.held_bytes(), 60);
    assert_eq!(gate.held_threads(), 1);
    let b = gate.try_take(40).expect("fits exactly");
    assert!(gate.try_take(0).is_none(), "threads would pass the limit");
    assert_eq!((gate.held_bytes(), gate.held_threads()), (100, 2));
    drop(a);
    drop(b);
    assert_eq!((gate.held_bytes(), gate.held_threads()), (0, 0));
}

#[test]
fn releasing_the_thread_keeps_the_bytes_until_the_grant_drops() {
    let gate = Gate::new(100, 1);
    let mut grant = gate.try_take(80).expect("fits");
    grant.release_thread();
    grant.release_thread();
    assert_eq!((gate.held_bytes(), gate.held_threads()), (80, 0));
    drop(grant);
    assert_eq!(gate.held_bytes(), 0);
}

#[test]
fn the_node_gate_is_one_gate() {
    assert!(std::sync::Arc::ptr_eq(&Gate::node(), &Gate::node()));
    assert_eq!(Gate::node().max_bytes(), 32 << 20);
    assert_eq!(Gate::node().max_threads(), 32);
}

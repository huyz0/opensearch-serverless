//! Early revocation's bounds (docs/specs/transport/SPEC.md, AC-15): the
//! single flight, the memory's bound, and the TCP probe on real sockets.

use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use oss_cluster::early_revoke::{Declined, MIN_RENEWED_AGO_MILLIS};
use oss_cluster::{ConnectProbe, EarlyRevoke, TcpProbe, Verdict};
use oss_format::NodeLease;

use crate::early::{ADDRESS, Probe, address, detector, settled};
use crate::{Fleet, TTL};

/// A probe that holds the first call until released.
#[derive(Default)]
struct Held {
    entered: Mutex<bool>,
    released: Mutex<bool>,
    changed: Condvar,
}

impl Held {
    fn wait_for(&self, flag: &Mutex<bool>) -> bool {
        let guard = flag.lock().unwrap();
        let (guard, _) = self
            .changed
            .wait_timeout_while(guard, Duration::from_secs(5), |set| !*set)
            .unwrap();
        *guard
    }

    fn set(&self, flag: &Mutex<bool>) {
        *flag.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

impl ConnectProbe for Held {
    fn refused(&self, _: SocketAddr) -> bool {
        self.set(&self.entered);
        self.wait_for(&self.released)
    }
}

#[test]
fn a_second_refusal_of_one_process_while_one_runs_declines_in_flight() {
    let fleet = Fleet::new();
    let _peer = settled(&fleet);
    let held = Arc::new(Held::default());
    let early = Arc::new(EarlyRevoke::new(Arc::new(fleet.node()), held.clone(), true));
    let now = fleet.clock.now();
    let first = {
        let early = early.clone();
        thread::spawn(move || early.on_refused("p", "ep", address(), now - 500))
    };
    assert!(held.wait_for(&held.entered), "the first reached its probe");
    let second = early.on_refused("p", "ep", address(), now - 500);
    held.set(&held.released);
    assert_eq!(second, Verdict::Declined(Declined::InFlight));
    assert_eq!(first.join().unwrap(), Verdict::Revoked);
}

#[test]
fn the_negative_cache_holds_at_most_max_members_and_then_declines() {
    let fleet = Fleet::new();
    let probe = Arc::new(Probe::default());
    let early = detector(&fleet, probe.clone(), true);
    // Sixteen peers (the fleet's max_members), each declined once.
    for i in 0..16 {
        let mut lease = NodeLease::new(&format!("p{i}"), "ep", ADDRESS, 0);
        lease.set_roles(["data".to_owned()]);
        fleet.node().renew(&lease).unwrap();
    }
    fleet.clock.advance(MIN_RENEWED_AGO_MILLIS + TTL / 32);
    let now = fleet.clock.now();
    for i in 0..16 {
        assert_eq!(
            early.on_refused(&format!("p{i}"), "ep", address(), now - 500),
            Verdict::Declined(Declined::Answered)
        );
    }
    let before = fleet.total();
    assert_eq!(
        early.on_refused("p-new", "ep", address(), now - 500),
        Verdict::Declined(Declined::Crowded)
    );
    assert_eq!(fleet.total(), before);
    // A lifetime on, the declines lapse and make room.
    fleet.clock.advance(TTL);
    assert_eq!(
        early.on_refused("p-new", "ep", address(), now),
        Verdict::Gone
    );
}

#[test]
fn the_tcp_probe_tells_a_refused_port_from_a_listening_one() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let open = listener.local_addr().unwrap();
    let closed = {
        let gone = TcpListener::bind("127.0.0.1:0").unwrap();
        gone.local_addr().unwrap()
    };
    let probe = TcpProbe::default();
    assert!(!probe.refused(open));
    assert!(probe.refused(closed));
}

#[test]
fn a_process_being_judged_counts_against_the_bound() {
    let fleet = Fleet::new();
    let _peer = settled(&fleet);
    let held = Arc::new(Held::default());
    let mut config = fleet.config(false);
    config.max_members = 1;
    let early = Arc::new(EarlyRevoke::new(
        Arc::new(fleet.node_with(config)),
        held.clone(),
        true,
    ));
    let now = fleet.clock.now();
    let first = {
        let early = early.clone();
        thread::spawn(move || early.on_refused("p", "ep", address(), now - 500))
    };
    assert!(held.wait_for(&held.entered), "the first reached its probe");
    let other = early.on_refused("q", "eq", address(), now - 500);
    held.set(&held.released);
    assert_eq!(other, Verdict::Declined(Declined::Crowded));
    assert_eq!(first.join().unwrap(), Verdict::Revoked);
}

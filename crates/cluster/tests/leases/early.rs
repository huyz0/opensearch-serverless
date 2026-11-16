//! Early revocation of a refused peer (docs/specs/transport/SPEC.md,
//! AC-14 and AC-15): every condition that declines, the revocation itself
//! and its requests, the negative cache and the single flight.

use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use oss_blob::S3Op;
use oss_cluster::early_revoke::{
    CONFIRM_AFTER_MILLIS, Declined, EARLY_REVOKE, MIN_RENEWED_AGO_MILLIS, SETTINGS,
};
use oss_cluster::{ClusterError, ConnectProbe, EarlyRevoke, Membership, TcpProbe, Verdict};
use oss_format::NodeLease;
use oss_types::settings::Registry;

use crate::{Fleet, Racing, TTL};

const ADDRESS: &str = "10.0.0.7:9300";

fn address() -> SocketAddr {
    ADDRESS.parse().unwrap()
}

/// The peer's lease, renewed now by its own membership.
fn peer_lease() -> NodeLease {
    let mut lease = NodeLease::new("p", "ep", ADDRESS, 0);
    lease.set_roles(["data".to_owned()]);
    lease
}

/// A probe answering `refused`, counting its calls.
#[derive(Default)]
struct Probe {
    answers_refused: AtomicBool,
    calls: AtomicU32,
}

impl Probe {
    fn refusing() -> Arc<Self> {
        let probe = Arc::new(Self::default());
        probe.answers_refused.store(true, Ordering::SeqCst);
        probe
    }
}

impl ConnectProbe for Probe {
    fn refused(&self, address: SocketAddr) -> bool {
        assert_eq!(address, self::address());
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.answers_refused.load(Ordering::SeqCst)
    }
}

/// A peer renewed at the fleet's start, a detector over the same store, and
/// the clock moved past the settle time.
fn settled(fleet: &Fleet) -> Membership {
    let peer = fleet.node();
    peer.renew(&peer_lease()).unwrap();
    fleet.clock.advance(MIN_RENEWED_AGO_MILLIS + TTL / 32);
    peer
}

fn detector(fleet: &Fleet, probe: Arc<Probe>, enabled: bool) -> EarlyRevoke {
    EarlyRevoke::new(Arc::new(fleet.node()), probe, enabled)
}

#[test]
fn the_setting_is_off_by_default_and_registers() {
    assert_eq!(EARLY_REVOKE.key(), "serverless.lease.early_revoke");
    assert!(!EARLY_REVOKE.default());
    Registry::new(&[SETTINGS]).unwrap();
}

#[test]
fn off_reads_nothing() {
    let fleet = Fleet::new();
    let _peer = settled(&fleet);
    let probe = Probe::refusing();
    let early = detector(&fleet, probe.clone(), false);
    assert!(!early.enabled());
    let before = fleet.total();
    let now = fleet.clock.now();
    assert_eq!(early.on_refused("p", "ep", address(), now), Verdict::Off);
    assert_eq!(fleet.total(), before);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
    assert!(!fleet.lease("p").unwrap().revoked);
}

#[test]
fn a_confirmed_refusal_revokes_in_one_read_and_one_swap_and_the_peer_cannot_renew() {
    let fleet = Fleet::new();
    let peer = settled(&fleet);
    let probe = Probe::refusing();
    let early = detector(&fleet, probe.clone(), true);
    assert!(early.enabled());
    let (gets, puts) = (fleet.count(S3Op::Get), fleet.count(S3Op::Put));
    let first = fleet.clock.now() - CONFIRM_AFTER_MILLIS;
    assert_eq!(
        early.on_refused("p", "ep", address(), first),
        Verdict::Revoked
    );
    assert_eq!(fleet.count(S3Op::Get) - gets, 1);
    assert_eq!(fleet.count(S3Op::Put) - puts, 1);
    assert_eq!(fleet.count(S3Op::List), 0);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        fleet.clock.pauses.load(Ordering::SeqCst),
        0,
        "500 ms had passed"
    );
    assert!(fleet.lease("p").unwrap().revoked);
    assert!(matches!(
        peer.renew(&peer_lease()),
        Err(ClusterError::Revoked { .. })
    ));
    // Revoked now: a later refusal finds it gone and writes nothing.
    let before = fleet.total();
    assert_eq!(early.on_refused("p", "ep", address(), first), Verdict::Gone);
    assert_eq!(fleet.count(S3Op::Put), puts + 1);
    assert_eq!(fleet.total() - before, 1);
}

#[test]
fn the_confirming_connect_waits_500_ms_after_the_first_refusal() {
    let fleet = Fleet::new();
    let _peer = settled(&fleet);
    let early = detector(&fleet, Probe::refusing(), true);
    let now = fleet.clock.now();
    assert_eq!(
        early.on_refused("p", "ep", address(), now - 100),
        Verdict::Revoked
    );
    assert_eq!(fleet.clock.pauses.load(Ordering::SeqCst), 1);
    assert_eq!(
        *fleet.clock.paused_for.lock().unwrap(),
        Duration::from_millis(400)
    );
}

#[test]
fn a_connect_that_is_not_refused_declines_and_is_remembered_for_a_lifetime() {
    let fleet = Fleet::new();
    let _peer = settled(&fleet);
    let probe = Arc::new(Probe::default());
    let early = detector(&fleet, probe.clone(), true);
    let first = fleet.clock.now() - CONFIRM_AFTER_MILLIS;
    assert_eq!(
        early.on_refused("p", "ep", address(), first),
        Verdict::Declined(Declined::Answered)
    );
    assert!(!fleet.lease("p").unwrap().revoked);
    let before = fleet.total();
    fleet.clock.advance(TTL - 1);
    assert_eq!(
        early.on_refused("p", "ep", address(), first),
        Verdict::Declined(Declined::RecentlyDeclined)
    );
    assert_eq!(fleet.total(), before, "the negative cache reads nothing");
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    // Another process of the node is not covered by the decline.
    assert_eq!(
        early.on_refused("p", "ep2", address(), first),
        Verdict::Gone
    );
    fleet.clock.advance(1);
    probe.answers_refused.store(true, Ordering::SeqCst);
    // A lifetime on, the peer's lease has expired in this test: gone.
    assert_eq!(early.on_refused("p", "ep", address(), first), Verdict::Gone);
}

#[test]
fn a_decline_expires_after_one_lifetime() {
    let fleet = Fleet::new();
    let peer = settled(&fleet);
    let probe = Arc::new(Probe::default());
    let early = detector(&fleet, probe.clone(), true);
    let first = fleet.clock.now() - CONFIRM_AFTER_MILLIS;
    assert_eq!(
        early.on_refused("p", "ep", address(), first),
        Verdict::Declined(Declined::Answered)
    );
    fleet.clock.advance(TTL);
    // The peer renewed meanwhile, long enough ago to settle.
    peer.renew(&peer_lease()).unwrap();
    fleet.clock.advance(MIN_RENEWED_AGO_MILLIS + TTL / 32);
    probe.answers_refused.store(true, Ordering::SeqCst);
    let now = fleet.clock.now();
    assert_eq!(
        early.on_refused("p", "ep", address(), now - CONFIRM_AFTER_MILLIS),
        Verdict::Revoked
    );
}

#[test]
fn a_lease_renewed_within_two_seconds_and_the_skew_margin_declines() {
    let fleet = Fleet::new();
    let peer = fleet.node();
    peer.renew(&peer_lease()).unwrap();
    fleet.clock.advance(MIN_RENEWED_AGO_MILLIS + TTL / 32 - 1);
    let probe = Probe::refusing();
    let early = detector(&fleet, probe.clone(), true);
    let now = fleet.clock.now();
    assert_eq!(
        early.on_refused("p", "ep", address(), now),
        Verdict::Declined(Declined::RenewedRecently)
    );
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
    // Declined, and so remembered: a moment later it is not tried.
    fleet.clock.advance(1);
    assert_eq!(
        early.on_refused("p", "ep", address(), now),
        Verdict::Declined(Declined::RecentlyDeclined)
    );
    // A fresh detector, one millisecond past the settle time, revokes.
    let early = detector(&fleet, probe, true);
    assert_eq!(
        early.on_refused("p", "ep", address(), now - 500),
        Verdict::Revoked
    );
}

#[test]
fn a_lease_of_another_process_none_or_an_expired_one_is_gone() {
    let fleet = Fleet::new();
    let _peer = settled(&fleet);
    let probe = Probe::refusing();
    let early = detector(&fleet, probe.clone(), true);
    let now = fleet.clock.now();
    assert_eq!(
        early.on_refused("p", "e-old", address(), now),
        Verdict::Gone
    );
    assert_eq!(
        early.on_refused("nobody", "e", address(), now),
        Verdict::Gone
    );
    fleet.clock.advance(TTL);
    assert_eq!(early.on_refused("p", "ep", address(), now), Verdict::Gone);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fleet.count(S3Op::Put), 2, "the peer's lease and index only");
}

#[test]
fn a_lease_at_another_address_declines() {
    let fleet = Fleet::new();
    let _peer = settled(&fleet);
    let early = detector(&fleet, Probe::refusing(), true);
    let elsewhere: SocketAddr = "10.0.0.8:9300".parse().unwrap();
    let now = fleet.clock.now();
    assert_eq!(
        early.on_refused("p", "ep", elsewhere, now),
        Verdict::Declined(Declined::AnotherAddress)
    );
    assert!(!fleet.lease("p").unwrap().revoked);
}

#[test]
fn a_renewal_between_the_read_and_the_swap_wins() {
    let fleet = Fleet::new();
    let peer = Arc::new(settled(&fleet));
    let racing = Racing::new(fleet.store.clone());
    {
        let peer = peer.clone();
        racing.set(move |_| {
            let _ = peer.renew(&peer_lease());
        });
    }
    let early = EarlyRevoke::new(Arc::new(fleet.node_on(racing)), Probe::refusing(), true);
    let now = fleet.clock.now();
    assert_eq!(
        early.on_refused("p", "ep", address(), now - 500),
        Verdict::Declined(Declined::Renewed)
    );
    assert!(!fleet.lease("p").unwrap().revoked);
}

#[test]
fn a_failing_store_or_an_unreadable_lease_declines() {
    let fleet = Fleet::new();
    let _peer = settled(&fleet);
    let early = detector(&fleet, Probe::refusing(), true);
    let now = fleet.clock.now();
    fleet.store.fail_next(S3Op::Get, 1);
    assert_eq!(
        early.on_refused("p", "ep", address(), now - 500),
        Verdict::Declined(Declined::Store)
    );
    fleet.store.fail_next(S3Op::Put, 1);
    assert_eq!(
        early.on_refused("p", "ep", address(), now - 500),
        Verdict::Declined(Declined::Store)
    );
    assert_eq!(
        early.on_refused("bad/id", "ep", address(), now - 500),
        Verdict::Declined(Declined::Store)
    );
    // A store failure is not remembered: the next refusal tries again.
    assert_eq!(
        early.on_refused("p", "ep", address(), now - 500),
        Verdict::Revoked
    );
}

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

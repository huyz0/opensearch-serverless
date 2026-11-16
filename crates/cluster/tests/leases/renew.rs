//! Renewal, enrolment, release and the self-validity checks (AC-3, AC-4).

use std::sync::atomic::Ordering;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_cluster::{ClusterError, LeaseConfig};

use crate::{BASE, Fleet, Racing, T0, TTL, lease};

#[test]
fn a_first_renewal_costs_three_requests_and_every_later_one_a_swap() {
    let fleet = Fleet::new();
    let node = fleet.node();
    assert!(!node.is_enrolled());
    let written = node.renew(&lease("n1")).unwrap();
    assert_eq!(written.expires_at_millis, T0 + TTL);
    assert_eq!(
        fleet.total(),
        3,
        "the lease, the index read, the index created"
    );
    assert_eq!((fleet.count(S3Op::Get), fleet.count(S3Op::Put)), (1, 2));
    assert!(node.is_enrolled());
    assert_eq!(fleet.index(), ["n1"]);
    assert_eq!(fleet.lease("n1").unwrap(), written);
    let before = fleet.total();
    for i in 1..=5 {
        fleet.clock.advance(10_000);
        let renewed = node.renew(&lease("n1")).unwrap();
        assert_eq!(renewed.expires_at_millis, T0 + i * 10_000 + TTL);
    }
    assert_eq!(fleet.total() - before, 5, "one swap a renewal");
    assert_eq!(fleet.count(S3Op::List), 0);
}

#[test]
fn a_second_node_joins_the_index_with_a_read_and_a_swap() {
    let fleet = Fleet::new();
    fleet.node().renew(&lease("n2")).unwrap();
    let before = fleet.total();
    fleet.node().renew(&lease("n1")).unwrap();
    assert_eq!(fleet.total() - before, 3);
    assert_eq!(fleet.index(), ["n1", "n2"]);
}

#[test]
fn a_revoked_lease_is_not_written_over_and_the_next_renewal_starts_afresh() {
    let fleet = Fleet::new();
    let node = fleet.node();
    node.renew(&lease("n1")).unwrap();
    fleet.clock.advance(TTL);
    let successor = fleet.node();
    assert!(successor.revoke("n1", Some("en1")));
    assert!(fleet.lease("n1").unwrap().revoked);
    let err = node.renew(&lease("n1")).unwrap_err();
    assert!(
        matches!(&err, ClusterError::Revoked { node_id } if node_id == "n1"),
        "{err}"
    );
    assert!(fleet.lease("n1").unwrap().revoked, "not written over");
    assert!(!node.is_enrolled());
    let renewed = node.renew(&lease("n1")).unwrap();
    assert!(!renewed.revoked);
    assert!(
        !fleet.lease("n1").unwrap().revoked,
        "afresh, over the revocation"
    );
    assert!(node.is_enrolled());
}

#[test]
fn a_live_lease_of_the_same_id_at_another_address_is_a_duplicate() {
    let fleet = Fleet::new();
    fleet.node().renew(&lease("n1")).unwrap();
    let mut clone = lease("n1");
    clone.ephemeral_id = "other".to_owned();
    clone.address = "elsewhere".to_owned();
    let err = fleet.node().renew(&clone).unwrap_err();
    assert!(
        matches!(&err, ClusterError::Duplicate { node_id, other_ephemeral_id, other_address }
            if node_id == "n1" && other_ephemeral_id == "en1" && other_address == "an1"),
        "{err}"
    );
    assert!(err.to_string().contains("duplicate node id [n1]"));
    assert_eq!(fleet.lease("n1").unwrap().ephemeral_id, "en1", "not taken");
}

#[test]
fn a_fast_restart_at_the_same_address_takes_its_lease_over() {
    let fleet = Fleet::new();
    fleet.node().renew(&lease("n1")).unwrap();
    let mut restarted = lease("n1");
    restarted.ephemeral_id = "e-new".to_owned();
    let node = fleet.node();
    let before = fleet.total();
    node.renew(&restarted).unwrap();
    // A create that lost, a read, a swap, and a forced swap of the index
    // (read and swap): the register moved underneath.
    assert_eq!(fleet.total() - before, 5);
    assert_eq!(fleet.lease("n1").unwrap().ephemeral_id, "e-new");
    assert!(node.is_enrolled());
}

#[test]
fn an_expired_lease_of_another_process_is_taken_over_anywhere() {
    let fleet = Fleet::new();
    fleet.node().renew(&lease("n1")).unwrap();
    fleet.clock.advance(TTL);
    let mut other = lease("n1");
    other.ephemeral_id = "other".to_owned();
    other.address = "elsewhere".to_owned();
    fleet.node().renew(&other).unwrap();
    assert_eq!(fleet.lease("n1").unwrap().address, "elsewhere");
}

#[test]
fn an_unreadable_lease_is_written_over() {
    let fleet = Fleet::new();
    let key = format!("{BASE}cluster/members/lease-n1");
    fleet.store.create_register(&key, b"not a lease").unwrap();
    fleet.node().renew(&lease("n1")).unwrap();
    assert_eq!(fleet.lease("n1").unwrap().ephemeral_id, "en1");
}

/// Moves the register at `key` to a new generation with the same value,
/// as another writer would.
fn bump(store: &Emulator, key: &str) {
    let mut buf = vec![0u8; 1 << 16];
    let read = store.read_register(key, &mut buf).unwrap().unwrap();
    store
        .swap_register(key, read.generation, &buf[..read.len])
        .unwrap();
}

#[test]
fn a_register_that_moves_again_under_the_retry_is_contended() {
    let fleet = Fleet::new();
    let racing = Racing::new(fleet.store.clone());
    let node = fleet.node_on(racing.clone());
    node.renew(&lease("n1")).unwrap();
    let inner = fleet.store.clone();
    let lease_key = format!("{BASE}cluster/members/lease-n1");
    racing.set(move |key| {
        if key == lease_key {
            bump(&inner, key);
        }
    });
    let err = node.renew(&lease("n1")).unwrap_err();
    assert!(matches!(err, ClusterError::Contended), "{err}");
    *racing.hook.lock().unwrap() = None;
    node.renew(&lease("n1")).unwrap();
}

#[test]
fn a_contended_enrolment_pauses_and_tries_again_on_the_next_renewal() {
    let fleet = Fleet::new();
    fleet.node().renew(&lease("n2")).unwrap();
    let paused = fleet.clock.pauses.load(Ordering::SeqCst);
    let racing = Racing::new(fleet.store.clone());
    let node = fleet.node_on(racing.clone());
    let inner = fleet.store.clone();
    let index_key = format!("{BASE}cluster/members/members");
    racing.set(move |key| {
        if key == index_key {
            bump(&inner, key);
        }
    });
    node.renew(&lease("n1")).unwrap();
    assert!(!node.is_enrolled(), "every attempt lost");
    assert_eq!(fleet.index(), ["n2"]);
    assert_eq!(
        fleet.clock.pauses.load(Ordering::SeqCst) - paused,
        2,
        "between the three attempts, not before the first"
    );
    *racing.hook.lock().unwrap() = None;
    let before = fleet.total();
    node.renew(&lease("n1")).unwrap();
    assert_eq!(
        fleet.total() - before,
        3,
        "the renewal, and the enrolment again"
    );
    node.renew(&lease("n1")).unwrap();
    assert_eq!(fleet.total() - before, 4, "enrolled: one swap");
    assert!(node.is_enrolled());
    assert_eq!(fleet.index(), ["n1", "n2"]);
}

#[test]
fn self_validity_follows_the_expiry_less_the_margin() {
    let fleet = Fleet::new();
    let node = fleet.node();
    assert_eq!(node.skew_margin_millis(), TTL / 32);
    assert!(
        node.self_valid_at(i64::MAX),
        "valid before the first renewal"
    );
    assert!(!node.self_lapsed_at(i64::MAX));
    assert_eq!(node.own_expires_at_millis(), 0);
    node.renew(&lease("n1")).unwrap();
    let edge = T0 + TTL - TTL / 32;
    assert!(node.self_valid_at(edge - 1));
    assert!(!node.self_valid_at(edge));
    assert!(!node.self_lapsed_at(edge - 1));
    assert!(node.self_lapsed_at(edge));
    node.set_skew_margin_millis(0).unwrap();
    assert!(node.self_valid_at(T0 + TTL - 1));
    assert!(node.self_lapsed_at(T0 + TTL));
    assert!(node.set_skew_margin_millis(TTL).is_err());
    assert!(node.set_skew_margin_millis(-1).is_err());
    assert!(node.set_skew_margin_millis(TTL - 1).is_ok());
}

#[test]
fn a_lifetime_that_is_not_positive_or_a_bad_base_is_refused() {
    let fleet = Fleet::new();
    for ttl in [0, -1] {
        let config = LeaseConfig {
            ttl_millis: ttl,
            ..fleet.config(false)
        };
        let store: std::sync::Arc<dyn oss_blob::BlobStore> = fleet.store.clone();
        let clock: std::sync::Arc<dyn oss_cluster::LeaseClock> = fleet.clock.clone();
        assert!(oss_cluster::Membership::new(store, BASE, clock, config).is_err());
    }
    let store: std::sync::Arc<dyn oss_blob::BlobStore> = fleet.store.clone();
    let clock: std::sync::Arc<dyn oss_cluster::LeaseClock> = fleet.clock.clone();
    assert!(oss_cluster::Membership::new(store, "d", clock, fleet.config(false)).is_err());
    assert_eq!(LeaseConfig::default().ttl_millis, 30_000);
    assert!(fleet.node().renew(&lease("a/b")).is_err());
}

#[test]
fn a_release_expires_the_lease_and_leaves_the_index() {
    let fleet = Fleet::new();
    let node = fleet.node();
    node.renew(&lease("n1")).unwrap();
    fleet.node().renew(&lease("n2")).unwrap();
    let before = fleet.total();
    node.release("n1").unwrap();
    assert_eq!(fleet.total() - before, 4, "two reads and two swaps");
    assert_eq!(fleet.lease("n1").unwrap().expires_at_millis, 0);
    assert_eq!(fleet.index(), ["n2"]);
    assert!(!node.is_enrolled());
    assert_eq!(node.own_expires_at_millis(), 0);
    assert!(node.current().is_empty());
}

#[test]
fn a_release_deletes_the_lease_only_where_conditional_deletes_are_verified() {
    let fleet = Fleet::new();
    let node = fleet.node_with(fleet.config(true));
    node.renew(&lease("n1")).unwrap();
    node.release("n1").unwrap();
    assert!(fleet.lease("n1").is_none());
    let mut logged = lease("n2");
    logged.node_log = vec![oss_format::NodeLogEpoch {
        epoch: 1,
        trim: 0,
        landed: 0,
    }];
    let other = fleet.node_with(fleet.config(true));
    other.renew(&logged).unwrap();
    other.release("n2").unwrap();
    assert!(
        fleet.lease("n2").is_some(),
        "kept while it names an epoch a successor reads"
    );
}

#[test]
fn a_release_leaves_another_process_lease_alone() {
    let fleet = Fleet::new();
    let node = fleet.node();
    node.renew(&lease("n1")).unwrap();
    fleet.clock.advance(TTL);
    let mut later = lease("n1");
    later.ephemeral_id = "later".to_owned();
    fleet.node().renew(&later).unwrap();
    node.release("n1").unwrap();
    let stored = fleet.lease("n1").unwrap();
    assert_eq!(stored.ephemeral_id, "later");
    assert!(stored.expires_at_millis > fleet.clock.now());
    fleet.node().release("absent").unwrap();
}

#[test]
fn the_defaults_are_the_javas_and_the_system_clock_moves() {
    use oss_cluster::LeaseClock;
    let config = oss_cluster::LeaseConfig::default();
    assert_eq!(config.ttl_millis, 30_000);
    assert_eq!(config.max_members, 512);
    assert!(!config.conditional_deletes);
    let fleet = Fleet::new();
    let node = fleet.node();
    assert_eq!(node.ttl_millis(), TTL);
    assert_eq!(node.own_expires_at_millis(), 0, "before the first renewal");
    node.renew(&lease("n1")).unwrap();
    assert_eq!(node.own_expires_at_millis(), T0 + TTL);
    let clock = oss_cluster::SystemClock;
    let before = clock.now_millis();
    assert!(before > 1_700_000_000_000, "{before}");
    clock.pause(std::time::Duration::from_millis(2));
    assert!(clock.now_millis() > before);
}

#[test]
fn a_node_the_index_already_names_enrols_without_a_swap() {
    let fleet = Fleet::new();
    let index = format!("{BASE}cluster/members/members");
    fleet.store.create_register(&index, b"n1").unwrap();
    let generation = fleet.read("cluster/members/members").unwrap().0;
    let node = fleet.node();
    let before = fleet.total();
    node.renew(&lease("n1")).unwrap();
    assert!(node.is_enrolled());
    assert_eq!(fleet.total() - before, 2, "the create and the index read");
    assert_eq!(fleet.read("cluster/members/members").unwrap().0, generation);
}

#[test]
fn an_error_names_the_store_or_format_error_behind_it() {
    use std::error::Error;
    let fleet = Fleet::new();
    let node = fleet.node();
    fleet.store.fail_next(S3Op::Put, 100);
    let blob = node.renew(&lease("n1")).unwrap_err();
    fleet.store.fail_next(S3Op::Put, 0);
    assert!(matches!(blob, ClusterError::Blob(_)), "{blob:?}");
    assert!(blob.source().is_some());
    let key = format!("{BASE}cluster/members/lease-n2");
    fleet.store.create_register(&key, b"[]").unwrap();
    let format = node.read("n2").unwrap_err();
    assert!(matches!(format, ClusterError::Format(_)), "{format:?}");
    assert!(format.source().is_some());
    assert!(ClusterError::Contended.source().is_none());
}

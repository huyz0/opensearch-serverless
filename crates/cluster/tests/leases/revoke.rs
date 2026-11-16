//! Revocation and the liveness oracle (AC-5).

use std::sync::Arc;

use oss_blob::{BlobStore, S3Op};
use oss_cluster::LivenessOracle;

use crate::{BASE, Fleet, TTL, lease};

#[test]
fn only_an_expired_lease_of_the_given_process_is_revoked() {
    let fleet = Fleet::new();
    fleet.node().renew(&lease("n1")).unwrap();
    let taker = fleet.node();
    let before = fleet.total();
    assert!(!taker.revoke("n1", Some("en1")), "live");
    assert_eq!(fleet.total() - before, 1, "one read, no swap");
    assert!(!fleet.lease("n1").unwrap().revoked);
    fleet.clock.advance(TTL - 1);
    assert!(!taker.revoke("n1", None), "live until the stamp, exactly");
    fleet.clock.advance(1);
    let before = fleet.total();
    assert!(taker.revoke("n1", Some("en1")));
    assert_eq!(fleet.total() - before, 2, "a read and a swap");
    assert!(fleet.lease("n1").unwrap().revoked);
    let before = fleet.total();
    assert!(taker.revoke("n1", Some("en1")), "already revoked");
    assert!(taker.revoke("n1", Some("e-later")), "a later process");
    assert!(taker.revoke("gone", None), "released or collected");
    assert_eq!(fleet.total() - before, 3, "reads only");
    assert_eq!(fleet.count(S3Op::List), 0);
}

#[test]
fn an_unreadable_lease_or_a_failing_store_is_not_revoked() {
    let fleet = Fleet::new();
    let key = format!("{BASE}cluster/members/lease-n1");
    fleet.store.create_register(&key, b"{").unwrap();
    let taker = fleet.node();
    assert!(!taker.revoke("n1", None));
    fleet.node().renew(&lease("n2")).unwrap();
    fleet.clock.advance(TTL);
    fleet.store.fail_next(S3Op::Get, 100);
    assert!(!taker.revoke("n2", None));
    fleet.store.fail_next(S3Op::Get, 0);
    assert!(!taker.revoke("a/b", None), "an id that cannot be a key");
}

#[test]
fn a_lost_swap_reads_again_and_decides_on_what_is_there() {
    let fleet = Fleet::new();
    let owner = fleet.node();
    owner.renew(&lease("n1")).unwrap();
    fleet.clock.advance(TTL);
    let racing = crate::Racing::new(fleet.store.clone());
    let taker = fleet.node_on(racing.clone());
    // The owner renews between the taker's read and its swap.
    let owner = Arc::new(owner);
    let renewing = Arc::clone(&owner);
    racing.set(move |_| {
        renewing.renew(&lease("n1")).unwrap();
    });
    assert!(!taker.revoke("n1", Some("en1")), "alive after all");
    assert!(!fleet.lease("n1").unwrap().revoked);
}

#[test]
fn concurrent_revocations_of_one_dead_node_share_one_swap() {
    let fleet = Fleet::new();
    fleet.node().renew(&lease("n1")).unwrap();
    fleet.clock.advance(TTL);
    let oracle = Arc::new(LivenessOracle::new(Arc::new(fleet.node())));
    let before = fleet.count(S3Op::Put);
    let answers: Vec<bool> = std::thread::scope(|s| {
        let running: Vec<_> = (0..8)
            .map(|_| {
                let oracle = Arc::clone(&oracle);
                s.spawn(move || oracle.revoke("n1", Some("en1")))
            })
            .collect();
        running.into_iter().map(|t| t.join().unwrap()).collect()
    });
    assert!(answers.iter().all(|a| *a), "{answers:?}");
    assert!(fleet.lease("n1").unwrap().revoked);
    assert_eq!(
        fleet.count(S3Op::Put) - before,
        1,
        "one swap between them, however they interleave"
    );
}

#[test]
fn the_oracle_reads_liveness_from_the_lease_itself() {
    let fleet = Fleet::new();
    let oracle = LivenessOracle::new(Arc::new(fleet.node()));
    assert!(!oracle.is_live("n1", None), "no lease");
    fleet.node().renew(&lease("n1")).unwrap();
    assert!(oracle.is_live("n1", None));
    assert!(oracle.is_live("n1", Some("en1")));
    assert!(!oracle.is_live("n1", Some("e-earlier")), "another process");
    fleet.clock.advance(TTL);
    assert!(!oracle.is_live("n1", None), "expired");
    fleet.store.fail_next(S3Op::Get, 100);
    assert!(oracle.is_live("n1", None), "unreadable is live");
    fleet.store.fail_next(S3Op::Get, 0);
    assert!(oracle.revoke("n1", Some("en1")));
    assert!(!oracle.is_live("n1", None), "revoked");
}

/// Eight revocations of `n1`, the first held at its swap long enough for
/// the rest to join it; `renew_meanwhile` has the owner renew there.
fn joined_revocations(renew_meanwhile: bool) -> Vec<bool> {
    let fleet = Fleet::new();
    let owner = Arc::new(fleet.node());
    owner.renew(&lease("n1")).unwrap();
    fleet.clock.advance(TTL);
    let racing = crate::Racing::new(fleet.store.clone());
    let lease_key = format!("{BASE}cluster/members/lease-n1");
    racing.set(move |key| {
        if key == lease_key {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if renew_meanwhile {
                owner.renew(&lease("n1")).unwrap();
            }
        }
    });
    let oracle = LivenessOracle::new(Arc::new(fleet.node_on(racing)));
    std::thread::scope(|s| {
        let running: Vec<_> = (0..8)
            .map(|_| s.spawn(|| oracle.revoke("n1", Some("en1"))))
            .collect();
        running.into_iter().map(|t| t.join().unwrap()).collect()
    })
}

#[test]
fn revocations_that_join_one_in_flight_take_its_answer() {
    assert_eq!(joined_revocations(false), [true; 8]);
    assert_eq!(joined_revocations(true), [false; 8]);
}

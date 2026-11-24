//! Throttling, revocation and the cost of the security snapshot
//! (docs/specs/security-authn/SPEC.md).

use crate::security_authn::{ADMIN, API, INTERVAL, Rig, call, make_user, rig, rig_over};

use std::sync::Arc;
use std::sync::atomic::Ordering;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_node::security_auth::{Authenticator, MAX_CACHED};

#[test]
fn five_wrong_passwords_throttle_that_user_only() {
    let r = rig();
    make_user(&r, "u", "Zz9!longEnough");
    make_user(&r, "v", "Yy8!longEnough");
    for _ in 0..5 {
        assert_eq!(
            call(
                &r,
                Some("u:bad"),
                "GET",
                "/_plugins/_security/authinfo",
                None
            )
            .0,
            401
        );
    }
    assert_eq!(
        call(
            &r,
            Some("u:bad"),
            "GET",
            "/_plugins/_security/authinfo",
            None
        )
        .0,
        429
    );
    assert_eq!(
        call(
            &r,
            Some("v:Yy8!longEnough"),
            "GET",
            "/_plugins/_security/authinfo",
            None
        )
        .0,
        200
    );
    r.now.fetch_add(10_001, Ordering::SeqCst);
    assert_eq!(
        call(
            &r,
            Some("u:Zz9!longEnough"),
            "GET",
            "/_plugins/_security/authinfo",
            None
        )
        .0,
        200
    );
}

#[test]
fn a_failed_check_is_never_cached() {
    let r = rig();
    make_user(&r, "u", "Zz9!longEnough");
    for _ in 0..2 {
        assert_eq!(
            call(
                &r,
                Some("u:bad"),
                "GET",
                "/_plugins/_security/authinfo",
                None
            )
            .0,
            401
        );
    }
    assert_eq!(
        call(
            &r,
            Some("u:Zz9!longEnough"),
            "GET",
            "/_plugins/_security/authinfo",
            None
        )
        .0,
        200
    );
    assert_eq!(
        call(
            &r,
            Some("u:bad"),
            "GET",
            "/_plugins/_security/authinfo",
            None
        )
        .0,
        401
    );
}

#[test]
fn a_full_verification_pool_answers_429() {
    use oss_node::security_auth::Refusal;
    let r = rig();
    make_user(&r, "u", "Zz9!longEnough");
    // No slot at all: a password that is not remembered cannot be checked.
    let full = Arc::new(
        Authenticator::new(Arc::clone(&r.state), Arc::clone(&r.clock)).with_limits(0, 4096),
    );
    assert_eq!(
        full.authenticate("u", "Zz9!longEnough"),
        Err(Refusal::Throttled)
    );
    // A slot frees when its check ends: one slot serves checks one after another.
    let one = Arc::new(
        Authenticator::new(Arc::clone(&r.state), Arc::clone(&r.clock)).with_limits(1, 4096),
    );
    assert_eq!(one.authenticate("u", "bad"), Err(Refusal::Unauthorized));
    assert!(one.authenticate("u", "Zz9!longEnough").is_ok());
}

#[test]
fn a_deleted_user_stops_authenticating_on_that_node_at_once() {
    let r = rig();
    make_user(&r, "u", "Zz9!longEnough");
    assert_eq!(
        call(
            &r,
            Some("u:Zz9!longEnough"),
            "GET",
            "/_plugins/_security/authinfo",
            None
        )
        .0,
        200
    );
    assert_eq!(
        call(
            &r,
            Some(ADMIN),
            "DELETE",
            &format!("{API}/internalusers/u"),
            None
        )
        .0,
        200
    );
    assert_eq!(
        call(
            &r,
            Some("u:Zz9!longEnough"),
            "GET",
            "/_plugins/_security/authinfo",
            None
        )
        .0,
        401
    );
    // A changed password: the old one stops, the new one starts.
    make_user(&r, "w", "Old!passwordX");
    assert_eq!(
        call(
            &r,
            Some("w:Old!passwordX"),
            "GET",
            "/_plugins/_security/authinfo",
            None
        )
        .0,
        200
    );
    make_user(&r, "w", "New!passwordX");
    assert_eq!(
        call(
            &r,
            Some("w:Old!passwordX"),
            "GET",
            "/_plugins/_security/authinfo",
            None
        )
        .0,
        401
    );
    assert_eq!(
        call(
            &r,
            Some("w:New!passwordX"),
            "GET",
            "/_plugins/_security/authinfo",
            None
        )
        .0,
        200
    );
}

#[test]
fn a_change_on_another_node_is_seen_after_the_interval() {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let a = rig_over(Arc::clone(&store));
    let b = rig_over(Arc::clone(&store));
    make_user(&a, "u", "Zz9!longEnough");
    assert_eq!(
        call(
            &b,
            Some("u:Zz9!longEnough"),
            "GET",
            "/_plugins/_security/authinfo",
            None
        )
        .0,
        200
    );
    assert_eq!(
        call(
            &a,
            Some(ADMIN),
            "DELETE",
            &format!("{API}/internalusers/u"),
            None
        )
        .0,
        200
    );
    assert_eq!(
        call(
            &b,
            Some("u:Zz9!longEnough"),
            "GET",
            "/_plugins/_security/authinfo",
            None
        )
        .0,
        200,
        "node b still holds the copy it took"
    );
    b.now.fetch_add(INTERVAL + 1, Ordering::SeqCst);
    assert_eq!(
        call(
            &b,
            Some("u:Zz9!longEnough"),
            "GET",
            "/_plugins/_security/authinfo",
            None
        )
        .0,
        401
    );
}

#[test]
fn the_credential_cache_is_bounded() {
    assert_eq!(MAX_CACHED, 4096);
    let r = rig();
    let small = Authenticator::new(Arc::clone(&r.state), Arc::clone(&r.clock)).with_limits(4, 2);
    for i in 0..3 {
        make_user(&r, &format!("u{i}"), "Zz9!longEnough");
    }
    for i in 0..3 {
        assert!(
            small
                .authenticate(&format!("u{i}"), "Zz9!longEnough")
                .is_ok()
        );
        assert!(small.remembered() <= 2, "after {i}: {}", small.remembered());
    }
}

#[test]
fn the_snapshot_refreshes_lazily_and_idle_nodes_make_no_requests() {
    let r = rig();
    let total = |r: &Rig| -> u64 { S3Op::ALL.iter().map(|op| r.store.meter().count(*op)).sum() };
    assert_eq!(
        call(&r, Some(ADMIN), "GET", "/_plugins/_security/authinfo", None).0,
        200
    );
    let idle = total(&r);
    r.now.fetch_add(10 * INTERVAL, Ordering::SeqCst);
    assert_eq!(total(&r), idle, "time passing alone makes no request");
    for _ in 0..5 {
        assert_eq!(
            call(&r, Some(ADMIN), "GET", "/_plugins/_security/authinfo", None).0,
            200
        );
    }
    let after = total(&r) - idle;
    assert!(
        after <= 5,
        "one refresh of five markers, not one a request: {after}"
    );
    let inside = total(&r);
    for _ in 0..5 {
        call(&r, Some(ADMIN), "GET", "/_plugins/_security/authinfo", None);
    }
    assert_eq!(total(&r), inside, "inside the interval nothing is read");
    assert_eq!(r.store.meter().count(S3Op::List), 0);
}

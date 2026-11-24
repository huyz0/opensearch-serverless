//! Clock, back-off and enumeration properties of authentication
//! (docs/specs/security-authn/SPEC.md).

use crate::security_authn::{ADMIN, INTERVAL, Rig, call, make_user, rig};

use std::sync::atomic::Ordering;

use oss_blob::emulator::faults::Faults;
use oss_blob::{BlobStore, S3Op};

const AUTHINFO: &str = "/_plugins/_security/authinfo";

fn gets(r: &Rig) -> u64 {
    r.store.meter().count(S3Op::Get)
}

fn store_down(r: &Rig) {
    r.store.set_faults(
        S3Op::Get,
        Faults {
            fail_ppm: 1_000_000,
            throttle_ppm: 0,
        },
    );
}

// docs/specs/security-authn AC-4
#[test]
fn a_clock_that_goes_back_does_not_keep_a_copy_fresh() {
    let r = rig();
    assert_eq!(call(&r, Some(ADMIN), "GET", AUTHINFO, None).0, 200);
    store_down(&r);
    r.now.fetch_sub(500_000, Ordering::SeqCst);
    let down = call(&r, Some(ADMIN), "GET", AUTHINFO, None);
    assert_eq!(down.0, 503, "{}", down.1);
}

// docs/specs/security-authn AC-4
#[test]
fn a_change_made_here_does_not_read_the_stores_on_every_request_while_they_are_down() {
    let r = rig();
    make_user(&r, "u", "Zz9!longEnough");
    assert_eq!(call(&r, Some(ADMIN), "GET", AUTHINFO, None).0, 200);
    let put = call(
        &r,
        Some(ADMIN),
        "PUT",
        &format!("{}/internalusers/v", crate::security_authn::API),
        Some(r#"{"password":"Zz9!longEnough"}"#),
    );
    assert!(put.0 == 200 || put.0 == 201, "{put:?}");
    store_down(&r);
    call(&r, Some(ADMIN), "GET", AUTHINFO, None);
    let before = gets(&r);
    for _ in 0..20 {
        call(&r, Some(ADMIN), "GET", AUTHINFO, None);
    }
    assert!(
        gets(&r) - before <= 5,
        "the retry back-off holds: {}",
        gets(&r) - before
    );
}

// docs/specs/security-authn AC-2
#[test]
fn an_unknown_name_is_throttled_like_a_known_one() {
    let r = rig();
    make_user(&r, "u", "Zz9!longEnough");
    let mut known = Vec::new();
    let mut unknown = Vec::new();
    for _ in 0..7 {
        known.push(call(&r, Some("u:wrong"), "GET", AUTHINFO, None).0);
        unknown.push(call(&r, Some("ghost:wrong"), "GET", AUTHINFO, None).0);
    }
    assert_eq!(
        known, unknown,
        "no answer tells a known name from an unknown"
    );
    assert_eq!(known.last(), Some(&429));
}

// docs/specs/security-authn AC-5
#[test]
fn a_change_made_here_is_seen_by_the_next_request_even_inside_the_interval() {
    let r = rig();
    make_user(&r, "u", "Zz9!longEnough");
    assert_eq!(
        call(&r, Some("u:Zz9!longEnough"), "GET", AUTHINFO, None).0,
        200
    );
    let del = call(
        &r,
        Some(ADMIN),
        "DELETE",
        &format!("{}/internalusers/u", crate::security_authn::API),
        None,
    );
    assert_eq!(del.0, 200, "{}", del.1);
    r.now.fetch_add(INTERVAL / 10, Ordering::SeqCst);
    assert_eq!(
        call(&r, Some("u:Zz9!longEnough"), "GET", AUTHINFO, None).0,
        401
    );
}

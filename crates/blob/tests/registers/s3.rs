//! AC-5 and the front end's conditional requests: a lost response to a
//! conditional PUT, and a second client's writes.

use oss_blob::S3Op::{Get, GetRange, Head, Put};
use oss_blob::{BlobStore, S3Op, SwapOutcome};

use crate::{client, counted, read, s3};

fn server(b: &crate::Backend) -> &oss_blob::emulator::http::S3Server {
    b.server.as_ref().unwrap()
}

// AC-5
#[test]
fn a_swap_whose_response_was_lost_finds_its_own_write() {
    let b = s3();
    let s = &*b.store;
    let SwapOutcome::Applied(g) = s.create_register("l/k", b"one").unwrap() else {
        panic!("create refused");
    };
    server(&b).store().lose_response_next(S3Op::Put, 1);
    let errors = s.meter().errors();
    let (outcome, n) = counted(s.meter(), || s.swap_register("l/k", g, b"two").unwrap());
    assert_eq!(outcome, SwapOutcome::Applied(g + 1));
    // The lost attempt, its retry refused by If-Match, one HEAD.
    assert_eq!(n, [(Head, 1), (Put, 2)]);
    assert_eq!(s.meter().errors() - errors, 2);
    // The ETag the HEAD read is remembered: the next swap sends one PUT.
    let (outcome, n) = counted(s.meter(), || {
        s.swap_register("l/k", g + 1, b"three").unwrap()
    });
    assert_eq!(outcome, SwapOutcome::Applied(g + 2));
    assert_eq!(n, [(Put, 1)]);
    assert_eq!(read(s, "l/k"), Some((g + 2, b"three".to_vec())));
}

// AC-5
#[test]
fn a_create_refused_on_retry_by_another_writers_register_is_a_conflict() {
    let b = s3();
    let s = &*b.store;
    let other = client(&server(&b).endpoint());
    other.create_register("l/k", b"theirs").unwrap();
    // The first attempt is refused, but the answer is lost; the retry is
    // refused too, and the HEAD finds another writer's token.
    server(&b).store().lose_response_next(S3Op::Put, 1);
    let (outcome, n) = counted(s.meter(), || s.create_register("l/k", b"mine").unwrap());
    assert_eq!(outcome, SwapOutcome::Conflict(None));
    assert_eq!(n, [(Head, 1), (Put, 2)]);
    assert_eq!(read(s, "l/k").unwrap().1, b"theirs");
}

// AC-4 across clients: each remembers only its own versions.
#[test]
fn two_clients_swap_one_register_without_losing_a_write() {
    let b = s3();
    let (a, z) = (&*b.store, client(&server(&b).endpoint()));
    let SwapOutcome::Applied(g) = a.create_register("two/k", b"a0").unwrap() else {
        panic!("create refused");
    };
    let (outcome, n) = counted(z.meter(), || z.swap_register("two/k", g, b"z1").unwrap());
    assert_eq!(outcome, SwapOutcome::Applied(g + 1));
    assert_eq!(n, [(GetRange, 1), (Put, 1)]);
    // `a` remembers g, which is stale: its If-Match PUT is refused.
    let (outcome, n) = counted(a.meter(), || a.swap_register("two/k", g, b"a1").unwrap());
    assert_eq!(outcome, SwapOutcome::Conflict(None));
    assert_eq!(n, [(Put, 1)]);
    let (seen, n) = counted(a.meter(), || read(a, "two/k"));
    assert_eq!(seen, Some((g + 1, b"z1".to_vec())));
    assert_eq!(n, [(Get, 1)]);
    // `z`'s delete with its remembered ETag is refused once `a` has written.
    a.swap_register("two/k", g + 1, b"a2").unwrap();
    assert_eq!(z.delete_register("two/k", g + 1), Ok(false));
    assert_eq!(a.delete_register("two/k", g + 2), Ok(true));
    assert_eq!(read(a, "two/k"), None);
    assert_eq!(server(&b).store().object_count(), 0);
}

// S3 answers 404 to `If-Match` when the object is gone: the version is not
// there, so the swap is a conflict, as the Java's `isConditionFailure`.
#[test]
fn a_swap_of_a_deleted_register_is_a_conflict() {
    let b = s3();
    let s = &*b.store;
    let SwapOutcome::Applied(g) = s.create_register("gone/k", b"v").unwrap() else {
        panic!("create refused");
    };
    // Deleted by another client: the remembered ETag's PUT gets S3's 404.
    let other = client(&server(&b).endpoint());
    other.delete("gone/k").unwrap();
    let (outcome, n) = counted(s.meter(), || s.swap_register("gone/k", g, b"w").unwrap());
    assert_eq!(outcome, SwapOutcome::Conflict(None));
    assert_eq!(n, [(Put, 1)]);
    assert_eq!(read(s, "gone/k"), None);
    assert_eq!(other.delete_register("gone/k", g), Ok(false));
    // A 404 to an unconditional PUT stays an error.
    server(&b).answer_next(404, "NoSuchKey", 1);
    assert_eq!(
        s.put(
            "plain",
            &mut oss_blob::SliceSource::new(b"x"),
            &mut [0u8; 8]
        ),
        Err(oss_blob::BlobError::NotFound)
    );
}

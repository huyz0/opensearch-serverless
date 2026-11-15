//! Failures and odd objects: a failed request forgets what the store
//! remembered and changes nothing; an empty object is no register.

use oss_blob::S3Op::{GetRange, Put};
use oss_blob::{BlobError, BlobStore, S3Op, SliceSource, SwapOutcome};

use crate::{Kind, all, counted, emulator, read, s3};

#[test]
fn a_failed_swap_changes_nothing_and_is_not_remembered() {
    for b in [emulator(), s3()] {
        let s = &*b.store;
        let faults = b.emulator.as_ref().unwrap();
        let SwapOutcome::Applied(g) = s.create_register("f/k", b"v").unwrap() else {
            panic!("create refused");
        };
        // The client tries 4 times; the emulator alone once.
        let attempts = if b.kind == Kind::S3 { 4 } else { 1 };
        faults.fail_next(S3Op::Put, attempts);
        assert_eq!(
            s.swap_register("f/k", g, b"w"),
            Err(BlobError::Unavailable),
            "{:?}",
            b.kind
        );
        let (outcome, n) = counted(s.meter(), || s.swap_register("f/k", g, b"w").unwrap());
        assert_eq!(outcome, SwapOutcome::Applied(g + 1), "{:?}", b.kind);
        assert_eq!(n, [(GetRange, 1), (Put, 1)], "{:?}: read afresh", b.kind);
        faults.fail_next(S3Op::GetRange, attempts);
        plant_other(s, "f/k");
        assert_eq!(
            s.swap_register("f/k", 9, b"x"),
            Err(BlobError::Unavailable),
            "{:?}",
            b.kind
        );
        assert_eq!(read(s, "f/k").unwrap().1, b"other", "{:?}", b.kind);
    }
}

/// Replaces the register with a plain PUT, so nothing is remembered.
fn plant_other(s: &dyn BlobStore, key: &str) {
    crate::plant(s, key, 9, b"other");
}

#[test]
fn an_empty_object_is_no_register() {
    for b in all() {
        let s = &*b.store;
        s.put("e/k", &mut SliceSource::new(b""), &mut [0u8; 8])
            .unwrap();
        assert_eq!(read(s, "e/k"), None, "{:?}", b.kind);
        let swapped = s.swap_register("e/k", 0, b"v");
        if b.kind == Kind::Fs {
            // The Java's filesystem swaps from it as from absent.
            assert!(
                matches!(swapped, Ok(SwapOutcome::Applied(_))),
                "{swapped:?}"
            );
        } else {
            // No generation to compare: refused, never overwritten blind.
            assert!(
                matches!(swapped, Err(BlobError::Protocol(_))),
                "{:?}: {swapped:?}",
                b.kind
            );
        }
    }
}

#[test]
fn a_directory_is_no_register_on_the_filesystem() {
    let b = crate::fs();
    let s = &*b.store;
    s.create_register("d/inner/r", b"v").unwrap();
    assert_eq!(read(s, "d/inner"), None);
    let (deleted, n) = counted(s.meter(), || s.delete_register("none/r", 5).unwrap());
    assert!(!deleted);
    assert_eq!(n, [(GetRange, 1)]);
    assert_eq!(s.delete_register("d/inner", 5), Ok(false));
}

//! Batch deletes: one request a thousand keys, every key checked first
//! (docs/specs/blob-batch-delete/SPEC.md).

use crate::{assert_counts, emulator, put};
use oss_blob::*;

fn keys(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("d/{i:05}")).collect()
}

// AC-1
#[test]
fn a_batch_delete_is_one_request_a_thousand_keys() {
    let e = emulator();
    let all = keys(2_500);
    for k in &all {
        put(&e, k, b"x");
    }
    let mut named: Vec<&str> = all.iter().map(String::as_str).collect();
    named.extend(["missing/1", "missing/2"]);
    e.delete_many(&named).unwrap();
    assert_eq!(e.object_count(), 0);
    assert_counts(e.meter(), &[(S3Op::Put, 2_500), (S3Op::Delete, 3)]);
    for (n, requests) in [(1, 1), (1_000, 1), (1_001, 2)] {
        let e = emulator();
        let all = keys(n);
        let named: Vec<&str> = all.iter().map(String::as_str).collect();
        e.delete_many(&named).unwrap();
        assert_counts(e.meter(), &[(S3Op::Delete, requests)]);
    }
}

// AC-2
#[test]
fn a_batch_delete_checks_every_key_before_sending_anything() {
    let e = emulator();
    put(&e, "a", b"x");
    let long = "k".repeat(MAX_KEY_BYTES + 1);
    for bad in ["", long.as_str()] {
        assert!(matches!(
            e.delete_many(&["a", bad]),
            Err(BlobError::InvalidArgument(_))
        ));
    }
    e.delete_many(&[]).unwrap();
    assert_eq!(e.object_count(), 1);
    assert_counts(e.meter(), &[(S3Op::Put, 1)]);
}

// AC-3
#[test]
fn a_failed_batch_leaves_the_keys_after_it_and_deleting_again_finishes() {
    let e = emulator();
    let all = keys(1_500);
    for k in &all {
        put(&e, k, b"x");
    }
    let named: Vec<&str> = all.iter().map(String::as_str).collect();
    e.fail_next(S3Op::Delete, 1);
    assert_eq!(e.delete_many(&named), Err(BlobError::Unavailable));
    assert_eq!(
        e.object_count(),
        1_500,
        "the first batch failed, so none went"
    );
    e.delete_many(&named).unwrap();
    assert_eq!(e.object_count(), 0);
    assert_eq!(e.meter().count(S3Op::Delete), 3);
}

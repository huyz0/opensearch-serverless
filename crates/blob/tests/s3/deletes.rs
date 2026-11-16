//! Batch deletes over HTTP: `POST /bucket?delete`, one request a thousand
//! keys, with the body's `Content-MD5` (docs/specs/blob-batch-delete/SPEC.md).

use crate::{assert_counts, fixture, put};
use oss_blob::*;

/// Keys an XML body must escape or carry as UTF-8.
const ODD: [&str; 4] = ["a&b<c>\"d'e", "ü/ ✓ space", "x]]>y", "tab\there"];

// AC-1, AC-4
#[test]
fn a_batch_delete_is_one_post_a_thousand_keys_on_client_and_server() {
    let f = fixture();
    let mut all: Vec<String> = (0..2_500).map(|i| format!("d/{i:05}")).collect();
    all.extend(ODD.iter().map(|k| (*k).to_owned()));
    for k in &all {
        put(f.server.store(), k, b"x");
    }
    put(f.server.store(), "kept", b"x");
    let before = f.server.requests();
    let mut named: Vec<&str> = all.iter().map(String::as_str).collect();
    named.push("never/there");
    f.client.delete_many(&named).unwrap();
    assert_eq!(f.server.store().object_count(), 1, "only the key not named");
    assert_counts(f.client.meter(), &[(S3Op::Delete, 3)]);
    assert_eq!(f.server.requests() - before, 3);
    assert_eq!(f.server.store().meter().count(S3Op::Delete), 3);
    assert_eq!(f.client.meter().errors(), 0);
}

// AC-2
#[test]
fn a_batch_delete_checks_every_key_before_any_request() {
    let f = fixture();
    let long = "k".repeat(MAX_KEY_BYTES + 1);
    for bad in ["", long.as_str()] {
        assert!(matches!(
            f.client.delete_many(&["a", bad]),
            Err(BlobError::InvalidArgument(_))
        ));
    }
    f.client.delete_many(&[]).unwrap();
    assert_eq!(f.server.requests(), 0);
    assert_eq!(f.client.meter().total(), 0);
}

// AC-3
#[test]
fn a_failed_batch_is_retried_and_each_attempt_counted() {
    let f = fixture();
    put(f.server.store(), "a", b"x");
    f.server.answer_next(503, "SlowDown", 2);
    f.client.delete_many(&["a"]).unwrap();
    assert_eq!(f.server.store().object_count(), 0);
    assert_counts(f.client.meter(), &[(S3Op::Delete, 3)]);
    assert_eq!(f.client.meter().errors(), 2);
    let f = fixture();
    put(f.server.store(), "a", b"x");
    f.server.answer_next(403, "AccessDenied", 1);
    assert_eq!(f.client.delete_many(&["a"]), Err(BlobError::Denied));
    assert_eq!(f.server.store().object_count(), 1);
    assert_counts(f.client.meter(), &[(S3Op::Delete, 1)]);
}

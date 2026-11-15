//! Retries, refusals and argument checks over HTTP.

use std::net::TcpListener;

use crate::{BUCKET, REGION, assert_counts, config, fixture, get_all, put};
use oss_blob::*;

// AC-8
#[test]
fn transient_statuses_are_retried_until_attempts_run_out() {
    for (status, code, err) in [
        (500, "InternalError", BlobError::Unavailable),
        (502, "BadGateway", BlobError::Unavailable),
        (503, "SlowDown", BlobError::Throttled),
        (504, "GatewayTimeout", BlobError::Unavailable),
        (429, "TooManyRequests", BlobError::Throttled),
    ] {
        let f = fixture();
        put(&f.client, "k", b"v");
        // Three failures, then the fourth attempt succeeds.
        f.server.answer_next(status, code, 3);
        assert_eq!(
            f.client.head("k"),
            Ok(Some(ObjectMeta { len: 1 })),
            "{status}"
        );
        // Four failures exhaust the four attempts.
        f.server.answer_next(status, code, 4);
        assert_eq!(f.client.delete("k"), Err(err), "{status}");
        assert_counts(
            f.client.meter(),
            &[(S3Op::Put, 1), (S3Op::Head, 4), (S3Op::Delete, 4)],
        );
        assert_eq!(f.client.meter().errors(), 7, "{status}");
        assert_eq!(f.server.requests(), 9);
    }
}

// AC-8
#[test]
fn refusals_are_not_retried() {
    for (status, err) in [
        (400, BlobError::Unexpected(400)),
        (401, BlobError::Denied),
        (403, BlobError::Denied),
        (404, BlobError::NotFound),
        (405, BlobError::Unexpected(405)),
        (416, BlobError::InvalidRange),
    ] {
        let f = fixture();
        f.server.answer_next(status, "Refused", 4);
        assert_eq!(
            get_all(&f.client, "k", ByteRange::Full),
            Err(err.clone()),
            "{status}"
        );
        f.server.answer_next(status, "Refused", 4);
        let mut n = 0;
        assert_eq!(
            // LIST: a test of the LIST page itself.
            f.client.list_prefix("", None, 1, &mut |_, _| n += 1),
            Err(err),
            "{status}"
        );
        assert_counts(f.client.meter(), &[(S3Op::Get, 1), (S3Op::List, 1)]);
        assert_eq!(f.client.meter().errors(), 2);
    }
}

// AC-8
#[test]
fn injected_store_faults_are_retried_and_each_attempt_counted() {
    let f = fixture();
    put(&f.client, "k", b"abc");
    f.server.store().throttle_next(S3Op::GetRange, 2);
    let r = ByteRange::From { start: 1, len: 2 };
    assert_eq!(get_all(&f.client, "k", r).unwrap(), b"bc");
    f.server.store().fail_next(S3Op::Get, 3);
    assert_eq!(get_all(&f.client, "k", ByteRange::Full).unwrap(), b"abc");
    assert_counts(
        f.client.meter(),
        &[(S3Op::Put, 1), (S3Op::GetRange, 3), (S3Op::Get, 4)],
    );
    assert_eq!(f.client.meter().errors(), 5);
    assert_eq!(f.server.store().meter().count(S3Op::Get), 4);
}

// AC-8
#[test]
fn a_connection_that_never_opens_is_retried_and_counted() {
    // A port that was free a moment ago and has no listener now.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let client = S3Store::new(config(
        &format!("http://127.0.0.1:{port}"),
        crate::credentials(),
    ))
    .unwrap();
    assert_eq!(client.head("k"), Err(BlobError::Unavailable));
    assert_eq!(
        client.put("k", &mut SliceSource::new(b"x"), &mut [0u8; 4]),
        Err(BlobError::Unavailable)
    );
    assert_counts(client.meter(), &[(S3Op::Head, 4), (S3Op::Put, 4)]);
    assert_eq!(client.meter().errors(), 8);
}

// AC-11
#[test]
fn a_request_signed_with_the_wrong_secret_is_denied_once() {
    let f = fixture();
    put(&f.client, "k", b"v");
    let wrong = S3Store::new(config(
        &f.server.endpoint(),
        Credentials::new("test-access", "not-the-secret"),
    ))
    .unwrap();
    assert_eq!(wrong.head("k"), Err(BlobError::Denied));
    assert_eq!(
        get_all(&wrong, "k", ByteRange::Full),
        Err(BlobError::Denied)
    );
    let mut other = config(&f.server.endpoint(), crate::credentials());
    other.region = "eu-west-1".into();
    let other = S3Store::new(other).unwrap();
    assert_eq!(other.delete("k"), Err(BlobError::Denied));
    assert_counts(wrong.meter(), &[(S3Op::Head, 1), (S3Op::Get, 1)]);
    assert_eq!(wrong.meter().errors(), 2);
    assert_eq!(other.meter().count(S3Op::Delete), 1);
    assert!(f.client.head("k").unwrap().is_some());
}

#[test]
fn a_bucket_the_server_does_not_hold_is_not_found() {
    let f = fixture();
    let mut c = config(&f.server.endpoint(), crate::credentials());
    c.bucket = "other-bucket".into();
    let other = S3Store::new(c).unwrap();
    assert_eq!(
        get_all(&other, "k", ByteRange::Full),
        Err(BlobError::NotFound)
    );
    assert_eq!(BUCKET, "test-bucket");
    assert_eq!(REGION, "us-east-1");
}

// AC-12
#[test]
fn invalid_arguments_fail_before_any_request() {
    let f = fixture();
    let long = "k".repeat(MAX_KEY_BYTES + 1);
    let mut sink = |_: &[u8]| Ok(());
    let c = &f.client;
    assert!(matches!(
        c.get("", ByteRange::Full, &mut [0; 4], &mut sink),
        Err(BlobError::InvalidArgument(_))
    ));
    assert!(matches!(
        c.get(&long, ByteRange::Full, &mut [0; 4], &mut sink),
        Err(BlobError::InvalidArgument(_))
    ));
    assert!(matches!(
        c.get("k", ByteRange::Full, &mut [], &mut sink),
        Err(BlobError::InvalidArgument(_))
    ));
    assert!(matches!(
        c.get(
            "k",
            ByteRange::From { start: 0, len: 0 },
            &mut [0; 4],
            &mut sink
        ),
        Err(BlobError::InvalidArgument(_))
    ));
    assert!(matches!(
        c.put("k", &mut SliceSource::new(b"x"), &mut []),
        Err(BlobError::InvalidArgument(_))
    ));
    assert!(matches!(
        c.create_if_absent("", &mut SliceSource::new(b"x"), &mut [0; 4]),
        Err(BlobError::InvalidArgument(_))
    ));
    assert!(matches!(c.head(""), Err(BlobError::InvalidArgument(_))));
    assert!(matches!(
        c.delete(&long),
        Err(BlobError::InvalidArgument(_))
    ));
    assert!(matches!(
        // LIST: a test of the LIST page itself.
        c.list_prefix("", None, 0, &mut |_, _| ()),
        Err(BlobError::InvalidArgument(_))
    ));
    assert_eq!(c.meter().total(), 0);
    assert_eq!(c.meter().errors(), 0);
    assert_eq!(f.server.requests(), 0);
}

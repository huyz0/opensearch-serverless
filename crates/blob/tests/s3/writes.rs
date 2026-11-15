//! PUTs and conditional creates over HTTP.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use crate::{BUCKET, REGION, assert_counts, config, credentials, fixture, get_all, put};
use oss_blob::emulator::http::S3Server;
use oss_blob::*;

/// A source that declares one length and yields another, or fails.
struct Lying {
    declared: u64,
    actual: usize,
    sent: usize,
    fail: bool,
    rewinds: bool,
}

impl ChunkSource for Lying {
    fn len(&self) -> u64 {
        self.declared
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        if self.fail && self.sent > 0 {
            return Err(BlobError::Stream("source broke"));
        }
        let n = self.actual.saturating_sub(self.sent).min(buf.len());
        buf[..n].fill(b'x');
        self.sent = self.sent.saturating_add(n);
        Ok(n)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        if !self.rewinds {
            return Err(BlobError::Stream("cannot rewind"));
        }
        self.sent = 0;
        Ok(())
    }
}

fn lying(declared: u64, actual: usize) -> Lying {
    Lying {
        declared,
        actual,
        sent: 0,
        fail: false,
        rewinds: true,
    }
}

// AC-4
#[test]
fn a_put_declares_its_length_and_never_chunks() {
    let f = fixture();
    put(&f.client, "k", &[7u8; 10_000]);
    put(&f.client, "empty", b"");
    let puts: Vec<_> = f
        .server
        .seen()
        .into_iter()
        .filter(|s| s.method == "PUT")
        .collect();
    assert_eq!(puts.len(), 2);
    assert_eq!(puts[0].header("content-length"), Some("10000"));
    assert_eq!(puts[1].header("content-length"), Some("0"));
    for p in &puts {
        assert_eq!(p.header("transfer-encoding"), None);
        assert_eq!(p.header("x-amz-content-sha256"), Some("UNSIGNED-PAYLOAD"));
        assert_eq!(p.header("if-none-match"), None);
    }
    assert_eq!(f.server.store().stored_bytes(), 10_000);
}

// AC-4
#[test]
fn a_body_of_the_wrong_length_stores_nothing() {
    let f = fixture();
    put(&f.client, "k", b"old");
    let mut buf = [0u8; 64];
    for (declared, actual) in [(100u64, 50usize), (100, 150), (100, 101), (0, 1), (64, 65)] {
        let err = f
            .client
            .put("k", &mut lying(declared, actual), &mut buf)
            .expect_err(&format!("{declared}/{actual}"));
        assert!(
            matches!(err, BlobError::InvalidArgument(_)),
            "{declared}/{actual}: {err:?}"
        );
        assert_eq!(get_all(&f.client, "k", ByteRange::Full).unwrap(), b"old");
    }
    let broken = Lying {
        fail: true,
        ..lying(100, 100)
    };
    assert_eq!(
        f.client.put("k", &mut { broken }, &mut buf),
        Err(BlobError::Stream("source broke"))
    );
    assert_eq!(get_all(&f.client, "k", ByteRange::Full).unwrap(), b"old");
    // Each bad body is one attempt and one error, never retried, except
    // a declared-empty body with bytes, refused before anything is sent.
    assert_eq!(f.client.meter().count(S3Op::Put), 6);
    assert_eq!(f.client.meter().errors(), 5);
    assert_eq!(f.client.meter().bytes_written(), 3);
}

// AC-5
#[test]
fn a_conditional_create_sends_if_none_match_and_never_overwrites() {
    let f = fixture();
    let mut buf = [0u8; 16];
    assert_eq!(
        f.client
            .create_if_absent("log/1", &mut SliceSource::new(b"first"), &mut buf),
        Ok(CreateOutcome::Created)
    );
    assert_eq!(
        f.client
            .create_if_absent("log/1", &mut SliceSource::new(b"second"), &mut buf),
        Ok(CreateOutcome::AlreadyExists)
    );
    assert_eq!(
        get_all(&f.client, "log/1", ByteRange::Full).unwrap(),
        b"first"
    );
    let seen = f.server.seen();
    assert!(
        seen.iter()
            .filter(|s| s.method == "PUT")
            .all(|s| s.header("if-none-match") == Some("*"))
    );
    assert_counts(f.client.meter(), &[(S3Op::Put, 2), (S3Op::Get, 1)]);
    assert_eq!(f.client.meter().errors(), 1);
}

// AC-5
#[test]
fn a_409_precondition_failed_is_taken_and_a_409_conflict_is_retried() {
    let f = fixture();
    let mut buf = [0u8; 16];
    f.server.answer_next(409, "PreconditionFailed", 1);
    assert_eq!(
        f.client
            .create_if_absent("a", &mut SliceSource::new(b"x"), &mut buf),
        Ok(CreateOutcome::AlreadyExists)
    );
    assert_eq!(f.client.meter().count(S3Op::Put), 1);

    f.server.answer_next(409, "ConditionalRequestConflict", 2);
    assert_eq!(
        f.client
            .create_if_absent("b", &mut SliceSource::new(b"x"), &mut buf),
        Ok(CreateOutcome::Created)
    );
    assert_eq!(f.client.meter().count(S3Op::Put), 4);

    f.server.answer_next(409, "ConditionalRequestConflict", 4);
    assert_eq!(
        f.client
            .create_if_absent("c", &mut SliceSource::new(b"x"), &mut buf),
        Err(BlobError::Conflict)
    );
    assert_eq!(f.client.meter().count(S3Op::Put), 8);
    assert_eq!(f.client.meter().errors(), 1 + 2 + 4);
    assert_eq!(f.server.store().object_count(), 1);
}

#[test]
fn a_412_on_a_plain_put_is_unexpected() {
    let f = fixture();
    f.server.answer_next(412, "PreconditionFailed", 1);
    assert_eq!(
        f.client
            .put("k", &mut SliceSource::new(b"x"), &mut [0u8; 4]),
        Err(BlobError::Unexpected(412))
    );
    assert_eq!(f.client.meter().count(S3Op::Put), 1);
}

// AC-5
#[test]
fn of_racing_creates_over_http_exactly_one_wins() {
    for round in 0..5u64 {
        let store = Arc::new(Emulator::new(1 << 20, round));
        let server = S3Server::start(store, BUCKET, REGION, credentials()).unwrap();
        let client = Arc::new(S3Store::new(config(&server.endpoint(), credentials())).unwrap());
        let barrier = Arc::new(Barrier::new(8));
        let wins = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..8u8)
            .map(|i| {
                let (client, barrier, wins) = (client.clone(), barrier.clone(), wins.clone());
                std::thread::spawn(move || {
                    let body = [i; 512];
                    barrier.wait();
                    let r = client
                        .create_if_absent("fence", &mut SliceSource::new(&body), &mut [0u8; 128])
                        .unwrap();
                    if r == CreateOutcome::Created {
                        wins.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(wins.load(Ordering::SeqCst), 1, "round {round}");
        assert_eq!(client.meter().count(S3Op::Put), 8);
        assert_eq!(client.meter().errors(), 7);
    }
}

// AC-10
#[test]
fn a_lost_response_to_a_create_is_retried_and_finds_its_own_object() {
    let f = fixture();
    // The store commits the first attempt, then the response is lost.
    f.server.store().lose_response_next(S3Op::Put, 1);
    assert_eq!(
        f.client
            .create_if_absent("log/7", &mut SliceSource::new(b"mine"), &mut [0u8; 8]),
        Ok(CreateOutcome::Created)
    );
    assert_eq!(
        get_all(&f.client, "log/7", ByteRange::Full).unwrap(),
        b"mine"
    );
    // The lost attempt and the 412 are failed attempts; one HEAD tells
    // whose object the 412 found.
    assert_counts(
        f.client.meter(),
        &[(S3Op::Put, 2), (S3Op::Head, 1), (S3Op::Get, 1)],
    );
    assert_eq!(f.client.meter().errors(), 2);
}

// AC-10
#[test]
fn a_retried_create_that_finds_another_writers_object_lost() {
    let f = fixture();
    // Another store with the same configuration, jitter seed included.
    let other = S3Store::new(config(&f.server.endpoint(), credentials())).unwrap();
    let mut buf = [0u8; 8];
    assert_eq!(
        other.create_if_absent("log/8", &mut SliceSource::new(b"theirs"), &mut buf),
        Ok(CreateOutcome::Created)
    );
    // This create's first attempt fails, so its 412 is checked.
    f.server.answer_next(503, "SlowDown", 1);
    assert_eq!(
        f.client
            .create_if_absent("log/8", &mut SliceSource::new(b"mine"), &mut buf),
        Ok(CreateOutcome::AlreadyExists)
    );
    assert_counts(f.client.meter(), &[(S3Op::Put, 2), (S3Op::Head, 1)]);
    // An object written without a token is never taken as this call's.
    put(f.server.store(), "log/9", b"x");
    f.server.answer_next(500, "InternalError", 1);
    assert_eq!(
        f.client
            .create_if_absent("log/9", &mut SliceSource::new(b"mine"), &mut buf),
        Ok(CreateOutcome::AlreadyExists)
    );
    assert_eq!(f.client.meter().count(S3Op::Head), 2);
}

// AC-10
#[test]
fn a_create_refused_on_its_first_attempt_needs_no_head() {
    let f = fixture();
    put(f.server.store(), "log/1", b"x");
    assert_eq!(
        f.client
            .create_if_absent("log/1", &mut SliceSource::new(b"y"), &mut [0u8; 8]),
        Ok(CreateOutcome::AlreadyExists)
    );
    assert_counts(f.client.meter(), &[(S3Op::Put, 1)]);
}

// AC-10
#[test]
fn a_failing_check_after_a_lost_response_reports_the_failure() {
    let f = fixture();
    f.server.store().lose_response_next(S3Op::Put, 1);
    f.server.store().fail_next(S3Op::Head, 4);
    assert_eq!(
        f.client
            .create_if_absent("log/2", &mut SliceSource::new(b"z"), &mut [0u8; 8]),
        Err(BlobError::Unavailable)
    );
    assert_counts(f.client.meter(), &[(S3Op::Put, 2), (S3Op::Head, 4)]);
}

// AC-9
#[test]
fn a_put_is_retried_only_when_its_body_rewinds() {
    let f = fixture();
    let mut buf = [0u8; 32];
    f.server.store().fail_next(S3Op::Put, 1);
    f.client.put("k", &mut lying(100, 100), &mut buf).unwrap();
    assert_eq!(f.client.meter().count(S3Op::Put), 2);
    assert_eq!(f.server.store().stored_bytes(), 100);

    f.server.store().fail_next(S3Op::Put, 1);
    let once = Lying {
        rewinds: false,
        ..lying(10, 10)
    };
    assert_eq!(
        f.client.put("j", &mut { once }, &mut buf),
        Err(BlobError::Unavailable)
    );
    assert_eq!(f.client.meter().count(S3Op::Put), 3);
    assert_eq!(f.client.meter().errors(), 2);
}

#[test]
fn a_store_out_of_space_is_unexpected_and_not_retried() {
    let store = Arc::new(Emulator::new(10, 1));
    let server = S3Server::start(store, BUCKET, REGION, credentials()).unwrap();
    let client = S3Store::new(config(&server.endpoint(), credentials())).unwrap();
    assert_eq!(
        client.put("k", &mut SliceSource::new(&[0u8; 11]), &mut [0u8; 4]),
        Err(BlobError::Unexpected(507))
    );
    assert_eq!(client.meter().count(S3Op::Put), 1);
    // The refused body was drained, so the connection still serves.
    assert_eq!(client.head("k"), Ok(None));
    assert_eq!(server.requests(), 2);
}

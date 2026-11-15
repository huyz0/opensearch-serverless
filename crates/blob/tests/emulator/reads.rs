//! GETs, HEADs and what every request records.

use crate::{MIB, assert_counts, emulator, get_all, pattern, put};
use oss_blob::*;

// AC-1
#[test]
fn each_operation_records_exactly_one_attempt_of_its_kind() {
    let e = emulator();
    put(&e, "a", b"hello");
    assert_counts(e.meter(), &[(S3Op::Put, 1)]);
    assert_eq!(get_all(&e, "a", ByteRange::Full).unwrap(), b"hello");
    assert_counts(e.meter(), &[(S3Op::Put, 1), (S3Op::Get, 1)]);
    get_all(&e, "a", ByteRange::From { start: 1, len: 2 }).unwrap();
    assert_eq!(
        e.create_if_absent("b", &mut SliceSource::new(b"x"), &mut [0; 8]),
        Ok(CreateOutcome::Created)
    );
    assert_eq!(e.head("a"), Ok(Some(ObjectMeta { len: 5 })));
    e.delete("a").unwrap();
    e.list_prefix("", None, 10, &mut |_, _| {}).unwrap(); // LIST: the operation under test
    assert_counts(
        e.meter(),
        &[
            (S3Op::Put, 2),
            (S3Op::Get, 1),
            (S3Op::GetRange, 1),
            (S3Op::Head, 1),
            (S3Op::Delete, 1),
            (S3Op::List, 1),
        ],
    );
    assert_eq!(e.meter().errors(), 0);
    assert_eq!(e.meter().bytes_written(), 6);
    assert_eq!(e.meter().bytes_read(), 7);
}

// AC-2
#[test]
fn a_failed_attempt_is_still_one_attempt_and_one_error() {
    let e = emulator();
    assert_eq!(
        get_all(&e, "missing", ByteRange::Full),
        Err(BlobError::NotFound)
    );
    assert_counts(e.meter(), &[(S3Op::Get, 1)]);
    assert_eq!(e.meter().errors(), 1);

    put(&e, "k", b"abc");
    assert_eq!(
        get_all(&e, "k", ByteRange::From { start: 3, len: 1 }),
        Err(BlobError::InvalidRange)
    );
    assert_eq!(
        e.create_if_absent("k", &mut SliceSource::new(b"z"), &mut [0; 8]),
        Ok(CreateOutcome::AlreadyExists)
    );
    e.fail_next(S3Op::Head, 1);
    assert_eq!(e.head("k"), Err(BlobError::Unavailable));
    e.throttle_next(S3Op::Delete, 1);
    assert_eq!(e.delete("k"), Err(BlobError::Throttled));
    assert_counts(
        e.meter(),
        &[
            (S3Op::Get, 1),
            (S3Op::GetRange, 1),
            (S3Op::Put, 2),
            (S3Op::Head, 1),
            (S3Op::Delete, 1),
        ],
    );
    assert_eq!(e.meter().errors(), 5);
    assert_eq!(
        e.head("missing"),
        Ok(None),
        "an absent key is an answer, not a failure"
    );
    assert_eq!(e.meter().errors(), 5);
}

#[test]
fn a_sink_failure_ends_the_get_and_counts_as_an_error() {
    let e = emulator();
    put(&e, "k", &pattern(5000));
    let mut calls = 0;
    let mut sink = |_: &[u8]| {
        calls += 1;
        Err(BlobError::Stream("client went away"))
    };
    let r = e.get("k", ByteRange::Full, &mut [0u8; 1000], &mut sink);
    assert_eq!(r, Err(BlobError::Stream("client went away")));
    assert_eq!(calls, 1);
    assert_eq!(e.meter().errors(), 1);
}

// AC-3
#[test]
fn a_get_streams_through_the_caller_buffer() {
    let e = emulator();
    let body = pattern(4 * MIB as usize);
    put(&e, "big", &body);
    let mut buf = vec![0u8; 64 * 1024];
    let (mut chunks, mut largest, mut out) = (0, 0, Vec::new());
    let mut sink = |c: &[u8]| {
        chunks += 1;
        largest = largest.max(c.len());
        out.extend_from_slice(c);
        Ok(())
    };
    let got = e.get("big", ByteRange::Full, &mut buf, &mut sink).unwrap();
    assert_eq!(got.bytes, 4 * MIB);
    assert_eq!(got.object_len, 4 * MIB);
    assert_eq!((chunks, largest), (64, 64 * 1024));
    assert!(out == body);
}

// AC-4
proptest::proptest! {
    #[test]
    fn a_ranged_get_returns_exactly_the_bytes_asked_for(len in 1usize..3000, start in 0u64..3500, want in 1u64..4000) {
        let e = emulator();
        let body = pattern(len);
        put(&e, "k", &body);
        let r = get_all(&e, "k", ByteRange::From { start, len: want });
        if start >= len as u64 {
            proptest::prop_assert_eq!(r, Err(BlobError::InvalidRange));
        } else {
            let end = start.saturating_add(want).min(len as u64) as usize;
            proptest::prop_assert_eq!(r.unwrap(), body[start as usize..end].to_vec());
        }
        proptest::prop_assert_eq!(e.meter().count(S3Op::GetRange), 1);
    }
}

#[test]
fn any_range_of_an_empty_object_is_invalid() {
    let e = emulator();
    put(&e, "empty", b"");
    assert_eq!(get_all(&e, "empty", ByteRange::Full).unwrap(), b"");
    assert_eq!(
        get_all(&e, "empty", ByteRange::From { start: 0, len: 1 }),
        Err(BlobError::InvalidRange)
    );
}

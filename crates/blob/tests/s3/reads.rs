//! GETs, HEADs and DELETEs over HTTP.

use crate::{assert_counts, fixture, get_all, pattern, put};
use oss_blob::*;

// AC-2
#[test]
fn each_operation_is_one_attempt_of_its_kind_and_one_request() {
    let f = fixture();
    let body = pattern(3000);
    put(&f.client, "seg/0", &body);
    assert_eq!(get_all(&f.client, "seg/0", ByteRange::Full).unwrap(), body);
    let part = get_all(&f.client, "seg/0", ByteRange::From { start: 10, len: 20 }).unwrap();
    assert_eq!(part, &body[10..30]);
    assert_eq!(
        f.client.head("seg/0").unwrap(),
        Some(ObjectMeta { len: 3000 })
    );
    f.client.delete("seg/0").unwrap();
    let mut keys = 0;
    f.client
        // LIST: a test of the LIST page itself.
        .list_prefix("seg/", None, 10, &mut |_, _| keys += 1)
        .unwrap();
    assert_eq!(keys, 0);
    assert_counts(
        f.client.meter(),
        &[
            (S3Op::Put, 1),
            (S3Op::Get, 1),
            (S3Op::GetRange, 1),
            (S3Op::Head, 1),
            (S3Op::Delete, 1),
            (S3Op::List, 1),
        ],
    );
    assert_eq!(f.server.requests(), 6);
    assert_eq!(f.server.store().meter().total(), 6);
    assert_eq!(f.client.meter().errors(), 0);
    assert_eq!(f.client.meter().bytes_written(), 3000);
    assert_eq!(f.client.meter().bytes_read(), 3020);
}

// AC-3
#[test]
fn a_ranged_get_sends_its_range_and_gets_exactly_those_bytes() {
    let f = fixture();
    let body = pattern(1000);
    put(&f.client, "k", &body);
    let cases = [
        (0u64, 1u64),
        (999, 1),
        (500, 500),
        (990, 100),
        (3, u64::MAX),
    ];
    for (start, len) in cases {
        let got = get_all(&f.client, "k", ByteRange::From { start, len }).unwrap();
        let end = start.saturating_add(len).min(1000) as usize;
        assert_eq!(got, &body[start as usize..end], "{start}+{len}");
    }
    let ranges: Vec<String> = f
        .server
        .seen()
        .iter()
        .filter_map(|s| s.header("range").map(str::to_owned))
        .collect();
    assert_eq!(
        ranges,
        [
            "bytes=0-0",
            "bytes=999-999",
            "bytes=500-999",
            "bytes=990-1089",
            "bytes=3-"
        ]
    );
}

// AC-3
#[test]
fn past_the_end_and_missing_keys_fail_in_one_attempt() {
    let f = fixture();
    put(&f.client, "k", b"abc");
    put(&f.client, "empty", b"");
    let past = ByteRange::From { start: 3, len: 1 };
    assert_eq!(get_all(&f.client, "k", past), Err(BlobError::InvalidRange));
    assert_eq!(
        get_all(&f.client, "empty", ByteRange::From { start: 0, len: 1 }),
        Err(BlobError::InvalidRange)
    );
    assert_eq!(get_all(&f.client, "empty", ByteRange::Full).unwrap(), b"");
    assert_eq!(
        get_all(&f.client, "nope", ByteRange::Full),
        Err(BlobError::NotFound)
    );
    assert_counts(
        f.client.meter(),
        &[(S3Op::Put, 2), (S3Op::GetRange, 2), (S3Op::Get, 2)],
    );
    assert_eq!(f.client.meter().errors(), 3);
}

// AC-6
#[test]
fn head_of_a_missing_key_is_none_and_delete_of_one_succeeds() {
    let f = fixture();
    assert_eq!(f.client.head("missing"), Ok(None));
    assert_eq!(f.client.delete("missing"), Ok(()));
    assert_counts(f.client.meter(), &[(S3Op::Head, 1), (S3Op::Delete, 1)]);
    assert_eq!(f.client.meter().errors(), 0);
}

// AC-9
#[test]
fn a_body_cut_part_way_is_unavailable_and_not_retried() {
    let f = fixture();
    let body = pattern(50_000);
    put(&f.client, "big", &body);
    f.server.cut_body_next(20_000);
    let mut seen = Vec::new();
    let err = f
        .client
        .get(
            "big",
            ByteRange::Full,
            &mut [0u8; 4096],
            &mut |c: &[u8]| {
                seen.extend_from_slice(c);
                Ok(())
            },
        )
        .unwrap_err();
    assert_eq!(err, BlobError::Unavailable);
    assert_eq!(seen, &body[..20_000]);
    assert_counts(f.client.meter(), &[(S3Op::Put, 1), (S3Op::Get, 1)]);
    assert_eq!(f.client.meter().errors(), 1);
    // The next GET opens a new connection and reads the whole object.
    assert_eq!(get_all(&f.client, "big", ByteRange::Full).unwrap(), body);
}

// AC-9
#[test]
fn a_body_cut_before_its_first_byte_is_retried() {
    let f = fixture();
    put(&f.client, "k", &pattern(100));
    f.server.cut_body_next(0);
    assert_eq!(
        get_all(&f.client, "k", ByteRange::Full).unwrap(),
        pattern(100)
    );
    assert_counts(f.client.meter(), &[(S3Op::Put, 1), (S3Op::Get, 2)]);
    assert_eq!(f.client.meter().errors(), 1);
}

#[test]
fn a_failing_sink_ends_the_get_without_a_retry() {
    let f = fixture();
    put(&f.client, "k", &pattern(10_000));
    let err = f
        .client
        .get("k", ByteRange::Full, &mut [0u8; 1024], &mut |_: &[u8]| {
            Err(BlobError::Stream("sink closed"))
        })
        .unwrap_err();
    assert_eq!(err, BlobError::Stream("sink closed"));
    assert_counts(f.client.meter(), &[(S3Op::Put, 1), (S3Op::Get, 1)]);
}

#[test]
fn chunks_never_exceed_the_caller_buffer() {
    let f = fixture();
    let body = pattern(100_000);
    put(&f.client, "k", &body);
    let mut sizes = Vec::new();
    let out = f
        .client
        .get("k", ByteRange::Full, &mut [0u8; 777], &mut |c: &[u8]| {
            sizes.push(c.len());
            Ok(())
        })
        .unwrap();
    assert_eq!(
        out,
        GetOutcome {
            object_len: 100_000,
            bytes: 100_000
        }
    );
    assert!(sizes.iter().all(|n| *n > 0 && *n <= 777));
    assert_eq!(sizes.iter().sum::<usize>(), 100_000);
}

#[test]
fn keys_with_reserved_characters_round_trip() {
    let f = fixture();
    let keys = [
        "a b/c+d=e&f?g#h",
        "ünïcödé/κλειδί",
        "100%/~tilde/.dot",
        "x//y",
    ];
    for key in keys {
        put(&f.client, key, key.as_bytes());
        assert_eq!(
            get_all(&f.client, key, ByteRange::Full).unwrap(),
            key.as_bytes()
        );
        assert!(f.client.head(key).unwrap().is_some());
    }
    assert_eq!(f.client.meter().errors(), 0);
}

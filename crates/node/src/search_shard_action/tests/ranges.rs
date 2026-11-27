//! The `range` and `date_range` aggregations of a forwarded search, answered.

use super::aggregations::ask_with;
use oss_reader::aggs::bucket::range::{RangeBucket, RangeResult};
use oss_reader::aggs::bucket::sub_values::BucketSubs;
use oss_reader::aggs::partial::{Format, Partial, Value as Part};
use oss_transport::actions::search_shard::{AggKind, AggRequest, RangeAsk, RangesRequest};

fn request(kind: AggKind) -> AggRequest {
    AggRequest {
        name: "r".to_owned(),
        kind,
        field: "n".to_owned(),
        precision_threshold: None,
        percentiles: None,
        terms: None,
        histogram: None,
        ranges: Some(RangesRequest {
            ranges: vec![RangeAsk {
                key: None,
                from: 0.0,
                from_text: None,
                to: 10.0,
                to_text: None,
            }],
            keyed: false,
        }),
        rare: None,
        multi: None,
        subs: Vec::new(),
    }
}

fn bucket(key: Option<&str>, from: f64, to: f64, doc_count: u64) -> RangeBucket {
    RangeBucket {
        key: key.map(str::to_owned),
        from,
        to,
        doc_count,
    }
}

fn part(format: Format, buckets: Vec<RangeBucket>, subs: BucketSubs) -> Partial {
    Partial {
        name: "r".to_owned(),
        format,
        value: Part::Range(Box::new(RangeResult {
            keyed: false,
            format: None,
            buckets,
            sub_meta: subs.meta,
            subs: subs.lists,
        })),
    }
}

fn contains(answer: &[u8], bytes: &[u8]) -> bool {
    answer.windows(bytes.len()).any(|w| w == bytes)
}

#[test]
fn a_range_is_answered_with_its_keys_bounds_and_counts() {
    let buckets = vec![
        bucket(Some("low"), f64::NEG_INFINITY, 10.0, 3),
        bucket(None, 10.0, f64::INFINITY, 5),
    ];
    let answer = ask_with(
        vec![request(AggKind::Range)],
        vec![part(Format::Raw, buckets, BucketSubs::default())],
    )
    .unwrap();
    // The name `range`, its name, no metadata, the raw format, not keyed, two
    // buckets; then the first bucket's key, bounds and count.
    let mut head = vec![1, 5];
    head.extend_from_slice(b"range");
    head.extend_from_slice(&[1, b'r', 0xFF, 3]);
    head.extend_from_slice(b"raw");
    head.extend_from_slice(&[0, 2, 3]);
    head.extend_from_slice(b"low");
    assert!(contains(&answer, &head), "{answer:?}");
    // The generated key of the second bucket.
    let mut generated = vec![6];
    generated.extend_from_slice(b"10.0-*");
    assert!(contains(&answer, &generated), "{answer:?}");
}

#[test]
fn a_date_range_is_answered_with_date_keys_and_the_date_format() {
    let buckets = vec![bucket(None, 1_000.0, 2_000.0, 2)];
    let answer = ask_with(
        vec![request(AggKind::DateRange)],
        vec![part(Format::Date, buckets, BucketSubs::default())],
    )
    .unwrap();
    let mut head = vec![1, 10];
    head.extend_from_slice(b"date_range");
    assert!(contains(&answer, &head), "{answer:?}");
    let key = b"1970-01-01T00:00:01.000Z-1970-01-01T00:00:02.000Z";
    let mut wire = vec![u8::try_from(key.len()).unwrap()];
    wire.extend_from_slice(key);
    assert!(contains(&answer, &wire), "{answer:?}");
    assert!(contains(&answer, b"date_time"), "{answer:?}");
}

#[test]
fn a_bucket_carries_its_sub_aggregations() {
    let mut req = request(AggKind::Range);
    req.subs = vec![AggRequest {
        name: "m".to_owned(),
        kind: AggKind::Max,
        field: "n".to_owned(),
        precision_threshold: None,
        percentiles: None,
        terms: None,
        histogram: None,
        ranges: None,
        rare: None,
        multi: None,
        subs: Vec::new(),
    }];
    let subs = BucketSubs {
        meta: vec![("m".to_owned(), Format::Raw)],
        empty: vec![Part::Max(f64::NEG_INFINITY)],
        lists: vec![vec![Part::Max(4.0)]],
    };
    let answer = ask_with(
        vec![req],
        vec![part(Format::Raw, vec![bucket(None, 0.0, 10.0, 3)], subs)],
    )
    .unwrap();
    let mut max = vec![1, 3, b'm', b'a', b'x', 1, b'm', 0xFF, 3, b'r', b'a', b'w'];
    max.extend_from_slice(&4.0f64.to_bits().to_be_bytes());
    assert!(contains(&answer, &max), "{answer:?}");
}

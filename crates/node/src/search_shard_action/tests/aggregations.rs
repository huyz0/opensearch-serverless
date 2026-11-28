//! The aggregations of a forwarded search, answered.

use super::*;

#[test]
fn aggregations_asked_for_come_back_as_internal_aggregations() {
    use oss_reader::aggs::partial::{Format, Partial, Value as Part};
    use oss_transport::actions::search_shard::{AggKind, AggRequest, Request as Wire, Source};
    let mut fake = Fake::held();
    fake.aggs = vec![
        Partial {
            name: "m".to_owned(),
            format: Format::Raw,
            value: Part::Max(2.0),
        },
        Partial {
            name: "c".to_owned(),
            format: Format::Raw,
            value: Part::ValueCount(7),
        },
    ];
    let h = handler(fake, &fresh(), 10_000);
    let mut ask_bytes = Wire::read(&fixture("request.match_all"), &Bounds::DEFAULT).unwrap();
    let source: &mut Source = &mut ask_bytes.source;
    source.aggregations = vec![
        AggRequest {
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
            wrap_longitude: true,
            subs: Vec::new(),
        },
        AggRequest {
            name: "c".to_owned(),
            kind: AggKind::ValueCount,
            field: "tag".to_owned(),
            precision_threshold: None,
            percentiles: None,
            terms: None,
            histogram: None,
            ranges: None,
            rare: None,
            multi: None,
            wrap_longitude: true,
            subs: Vec::new(),
        },
    ];
    let mut message = Vec::new();
    ask_bytes
        .write(&mut Output::new(&mut message, 1 << 20))
        .unwrap();
    let answer = ask(&h, &message, WireVersion::SHELL).unwrap();
    let mut expected = vec![
        1, 2, 3, b'm', b'a', b'x', 1, b'm', 0xFF, 3, b'r', b'a', b'w',
    ];
    expected.extend_from_slice(&2.0f64.to_bits().to_be_bytes());
    expected.push(11);
    expected.extend_from_slice(b"value_count");
    expected.extend_from_slice(&[1, b'c', 0xFF, 7]);
    assert!(
        answer.windows(expected.len()).any(|w| w == &expected[..]),
        "{answer:?}"
    );
}

#[test]
fn a_date_aggregation_carries_the_date_format_and_another_kind_is_refused() {
    use oss_reader::aggs::partial::{Format, Partial, Value as Part};
    let mut fake = Fake::held();
    fake.aggs = vec![Partial {
        name: "d".to_owned(),
        format: Format::Date,
        value: Part::Max(1.0),
    }];
    let h = handler(fake, &fresh(), 10_000);
    let answer = ask(&h, &fixture("request.match_all"), WireVersion::SHELL).unwrap();
    assert!(answer.windows(9).any(|w| w == b"date_time"), "{answer:?}");
    let mut fake = Fake::held();
    fake.aggs = vec![Partial {
        name: "t".to_owned(),
        format: Format::Raw,
        value: Part::Rendered(Vec::new()),
    }];
    let h = handler(fake, &fresh(), 10_000);
    let e = ask(&h, &fixture("request.match_all"), WireVersion::SHELL).unwrap_err();
    assert!(
        format!("{e:?}").contains("not supported by this node"),
        "{e:?}"
    );
}

#[test]
fn a_stats_aggregation_is_answered_with_its_count_min_max_and_sum() {
    use oss_reader::aggs::partial::{Format, Partial, Value as Part};
    let mut fake = Fake::held();
    fake.aggs = vec![Partial {
        name: "st".to_owned(),
        format: Format::Raw,
        value: Part::Stats {
            count: 3,
            sum: 6.0,
            min: 1.0,
            max: 4.0,
        },
    }];
    let h = handler(fake, &fresh(), 10_000);
    let answer = ask(&h, &fixture("request.match_all"), WireVersion::SHELL).unwrap();
    let mut expected = vec![1, 5, b's', b't', b'a', b't', b's', 2, b's', b't', 0xFF];
    expected.extend_from_slice(&[3, b'r', b'a', b'w', 3]);
    for v in [1.0f64, 4.0, 6.0] {
        expected.extend_from_slice(&v.to_bits().to_be_bytes());
    }
    assert!(
        answer.windows(expected.len()).any(|w| w == &expected[..]),
        "{answer:?}"
    );
}

#[test]
fn a_cardinality_aggregation_is_answered_with_its_sketch() {
    use oss_reader::aggs::metric::hll::Hll;
    use oss_reader::aggs::partial::{Format, Partial, Value as Part};
    let sketch = Hll::read(&[4, 0, 0]).expect("an empty sketch");
    let mut fake = Fake::held();
    fake.aggs = vec![Partial {
        name: "u".to_owned(),
        format: Format::Raw,
        value: Part::Cardinality(Some(sketch)),
    }];
    let h = handler(fake, &fresh(), 10_000);
    let answer = ask(&h, &fixture("request.match_all"), WireVersion::SHELL).unwrap();
    let mut expected = vec![1, 11];
    expected.extend_from_slice(b"cardinality");
    expected.extend_from_slice(&[1, b'u', 0xFF, 3, b'r', b'a', b'w', 1, 4, 0, 0]);
    assert!(
        answer.windows(expected.len()).any(|w| w == &expected[..]),
        "{answer:?}"
    );
}

#[test]
fn a_percentiles_aggregation_is_answered_with_its_digest_and_an_empty_one_with_the_compression() {
    use oss_reader::aggs::metric::tdigest::Digest;
    use oss_reader::aggs::partial::{Format, Partial, Value as Part};
    let part = |digest| Partial {
        name: "p".to_owned(),
        format: Format::Raw,
        value: Part::Percentiles {
            ranks: false,
            keys: vec![50.0],
            keyed: true,
            compression: 100.0,
            digest,
        },
    };
    for digest in [None, Some(Digest::new(100.0))] {
        let mut fake = Fake::held();
        fake.aggs = vec![part(digest)];
        let h = handler(fake, &fresh(), 10_000);
        let answer = ask(&h, &fixture("request.match_all"), WireVersion::SHELL).unwrap();
        let mut expected = vec![1, 19];
        expected.extend_from_slice(b"tdigest_percentiles");
        expected.extend_from_slice(&[1, b'p', 0xFF, 3, b'r', b'a', b'w', 1]);
        expected.extend_from_slice(&50.0f64.to_bits().to_be_bytes());
        expected.extend_from_slice(&Digest::new(100.0).to_bytes().len().to_be_bytes()[7..]);
        assert!(
            answer.windows(expected.len()).any(|w| w == &expected[..]),
            "{answer:?}"
        );
    }
}

fn terms_request(
    name: &str,
    order: oss_transport::actions::search_shard::Order,
) -> oss_transport::actions::search_shard::AggRequest {
    use oss_transport::actions::search_shard::{AggKind, AggRequest, OrderSpec, TermsRequest};
    AggRequest {
        name: name.to_owned(),
        kind: AggKind::Terms,
        field: "tag".to_owned(),
        precision_threshold: None,
        percentiles: None,
        terms: Some(TermsRequest {
            size: 10,
            shard_size: -1,
            min_doc_count: 1,
            shard_min_doc_count: 0,
            order: OrderSpec {
                compound: false,
                items: vec![order],
            },
            show_error: false,
        }),
        histogram: None,
        ranges: None,
        rare: None,
        multi: None,
        wrap_longitude: true,
        subs: Vec::new(),
    }
}

pub(super) fn ask_with(
    aggs: Vec<oss_transport::actions::search_shard::AggRequest>,
    parts: Vec<oss_reader::aggs::partial::Partial>,
) -> Result<Vec<u8>, oss_transport::exception::RemoteException> {
    use oss_transport::actions::search_shard::Request as Wire;
    let mut fake = Fake::held();
    fake.aggs = parts;
    let h = handler(fake, &fresh(), 10_000);
    let mut wire = Wire::read(&fixture("request.match_all"), &Bounds::DEFAULT).unwrap();
    wire.source.aggregations = aggs;
    let mut message = Vec::new();
    wire.write(&mut Output::new(&mut message, 1 << 20)).unwrap();
    ask(&h, &message, WireVersion::SHELL)
}

#[test]
fn a_keyword_terms_aggregation_is_answered_as_string_terms_in_the_requests_order() {
    use oss_reader::aggs::bucket::keyword_terms::KeywordTermsResult;
    use oss_reader::aggs::bucket::terms::TermsSpec;
    use oss_reader::aggs::partial::{Format, Partial, Value as Part};
    use oss_transport::actions::search_shard::Order;
    let part = Partial {
        name: "t".to_owned(),
        format: Format::Raw,
        value: Part::KeywordTerms(Box::new(KeywordTermsResult {
            spec: TermsSpec::default(),
            other_doc_count: 2,
            doc_count_error: 0,
            buckets: vec![("a".to_owned(), 3), ("b".to_owned(), 1)],
            bucket_errors: Vec::new(),
            sub_meta: Vec::new(),
            subs: Vec::new(),
        })),
    };
    let answer = ask_with(vec![terms_request("t", Order::CountDesc)], vec![part]).unwrap();
    // The type, the name, no metadata, the reduce order (by key), the
    // request's order (by count), the size, the minimum count, and the
    // error, then `raw`.
    let mut expected = vec![1, 6];
    expected.extend_from_slice(b"sterms");
    expected.extend_from_slice(&[1, b't', 0xFF, 4, 1, 10, 1, 0, 3, b'r', b'a', b'w']);
    assert!(
        answer.windows(expected.len()).any(|w| w == &expected[..]),
        "{answer:?}"
    );
    // Its buckets end the answer's aggregations: a count, a nothing and a term.
    let tail = [3u8, 0, 1, b'a', 1, 0, 1, b'b'];
    assert!(answer.windows(tail.len()).any(|w| w == tail), "{answer:?}");
}

#[test]
fn a_numeric_terms_aggregation_is_answered_as_long_terms_and_a_bucket_carries_its_sub_aggregations()
{
    use oss_reader::aggs::bucket::terms::{TermsResult, TermsSpec};
    use oss_reader::aggs::partial::{Format, Partial, Value as Part};
    use oss_transport::actions::search_shard::Order;
    let part = |meta: Vec<(String, Format)>, lists: Vec<Vec<Part>>| Partial {
        name: "u".to_owned(),
        format: Format::Raw,
        value: Part::Terms(Box::new(TermsResult {
            spec: TermsSpec::default(),
            buckets: vec![(7, 3)],
            subs: oss_reader::aggs::bucket::sub_values::BucketSubs {
                meta,
                lists,
                ..Default::default()
            },
            ..Default::default()
        })),
    };
    let answer = ask_with(
        vec![terms_request("u", Order::KeyAsc)],
        vec![part(Vec::new(), Vec::new())],
    )
    .unwrap();
    let mut expected = vec![1, 6];
    expected.extend_from_slice(b"lterms");
    assert!(
        answer.windows(expected.len()).any(|w| w == &expected[..]),
        "{answer:?}"
    );
    let tail = [3u8, 0, 0, 0, 0, 0, 0, 0, 0, 7];
    assert!(
        answer.ends_with(&[1, 3, 0, 0, 0, 0, 0, 0, 0, 0, 7])
            || answer.windows(tail.len()).any(|w| w == tail),
        "{answer:?}"
    );
    // A bucket's sub-aggregation, as its own named writeable before the key.
    let mut sub_request = terms_request("u", Order::KeyAsc);
    sub_request.subs = vec![oss_transport::actions::search_shard::AggRequest {
        name: "m".to_owned(),
        kind: oss_transport::actions::search_shard::AggKind::Max,
        field: "n".to_owned(),
        precision_threshold: None,
        percentiles: None,
        terms: None,
        histogram: None,
        ranges: None,
        rare: None,
        multi: None,
        wrap_longitude: true,
        subs: Vec::new(),
    }];
    let meta = vec![("m".to_owned(), Format::Raw)];
    let answer = ask_with(
        vec![sub_request.clone()],
        vec![part(meta.clone(), vec![vec![Part::Max(4.0)]])],
    )
    .unwrap();
    let mut expected = vec![1, 3, b'm', b'a', b'x', 1, b'm', 0xFF, 3, b'r', b'a', b'w'];
    expected.extend_from_slice(&4.0f64.to_bits().to_be_bytes());
    assert!(
        answer.windows(expected.len()).any(|w| w == &expected[..]),
        "{answer:?}"
    );
    // A sub-aggregation result this node does not answer is refused.
    let refused = ask_with(
        vec![sub_request],
        vec![part(meta, vec![vec![Part::Rendered(Vec::new())]])],
    )
    .unwrap_err();
    assert!(
        format!("{refused:?}").contains("not supported by this node"),
        "{refused:?}"
    );
}

fn histogram_request(
    interval: &str,
    interval_type: oss_transport::actions::search_shard::IntervalType,
) -> oss_transport::actions::search_shard::AggRequest {
    use oss_transport::actions::search_shard::{
        AggKind, AggRequest, DateHistogramRequest, Order, OrderSpec,
    };
    AggRequest {
        name: "d".to_owned(),
        kind: AggKind::DateHistogram,
        field: "t".to_owned(),
        precision_threshold: None,
        percentiles: None,
        terms: None,
        histogram: Some(DateHistogramRequest {
            order: OrderSpec {
                compound: false,
                items: vec![Order::KeyAsc],
            },
            keyed: false,
            min_doc_count: 0,
            interval: interval.to_owned(),
            interval_type,
            offset: 0,
        }),
        ranges: None,
        rare: None,
        multi: None,
        wrap_longitude: true,
        subs: Vec::new(),
    }
}

#[test]
fn a_date_histogram_is_answered_with_its_rounding_buckets_and_empty_bucket_sub_aggregations() {
    use oss_reader::aggs::bucket::date_histogram::{DateHistogramResult, DateUnit, RoundingKind};
    use oss_reader::aggs::bucket::sub_values::BucketSubs;
    use oss_reader::aggs::partial::{Format, Partial, Value as Part};
    use oss_transport::actions::search_shard::{AggKind, AggRequest, IntervalType};
    let mut spec = oss_reader::aggs::bucket::date_histogram::DateHistogramSpec::default();
    spec.rounding.kind = RoundingKind::Unit(DateUnit::Day);
    let part = |subs: BucketSubs| Partial {
        name: "d".to_owned(),
        format: Format::Date,
        value: Part::DateHistogram(Box::new(DateHistogramResult {
            spec: spec.clone(),
            buckets: vec![(86_400_000, 3), (172_800_000, 1)],
            subs,
        })),
    };
    let answer = ask_with(
        vec![histogram_request("1d", IntervalType::Calendar)],
        vec![part(BucketSubs::default())],
    )
    .unwrap();
    // The name, the key order, no minimum, the day rounding in UTC, then no
    // empty sub-aggregations and no bounds.
    let mut expected = vec![1, 14];
    expected.extend_from_slice(b"date_histogram");
    expected.extend_from_slice(&[1, b'd', 0xFF, 4, 0, 1, 5, 1, b'Z', 0, 0]);
    assert!(
        answer.windows(expected.len()).any(|w| w == &expected[..]),
        "{answer:?}"
    );
    // A sub-aggregation, and what an empty bucket holds of it.
    let mut request = histogram_request("1d", IntervalType::Calendar);
    request.subs = vec![AggRequest {
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
        wrap_longitude: true,
        subs: Vec::new(),
    }];
    let subs = BucketSubs {
        meta: vec![("m".to_owned(), Format::Raw)],
        empty: vec![Part::Max(f64::NEG_INFINITY)],
        lists: vec![vec![Part::Max(4.0)], vec![Part::Max(f64::NEG_INFINITY)]],
    };
    let answer = ask_with(vec![request], vec![part(subs)]).unwrap();
    let mut empty = vec![1, 3, b'm', b'a', b'x', 1, b'm', 0xFF, 3, b'r', b'a', b'w'];
    empty.extend_from_slice(&f64::NEG_INFINITY.to_bits().to_be_bytes());
    assert!(
        answer.windows(empty.len()).any(|w| w == &empty[..]),
        "{answer:?}"
    );
}

#[test]
fn a_date_histogram_in_a_time_zone_is_refused() {
    use oss_reader::aggs::bucket::date_histogram::{DateHistogramResult, DateHistogramSpec};
    use oss_reader::aggs::partial::{Format, Partial, Value as Part};
    use oss_transport::actions::search_shard::IntervalType;
    let mut spec = DateHistogramSpec::default();
    spec.rounding.zone_ms = 3_600_000;
    let part = Partial {
        name: "d".to_owned(),
        format: Format::Date,
        value: Part::DateHistogram(Box::new(DateHistogramResult {
            spec,
            buckets: Vec::new(),
            subs: Default::default(),
        })),
    };
    let refused = ask_with(
        vec![histogram_request("1d", IntervalType::Calendar)],
        vec![part],
    )
    .unwrap_err();
    assert!(format!("{refused:?}").contains("time zone"), "{refused:?}");
}

#[test]
fn a_terms_over_a_double_field_is_answered_as_double_terms_with_the_decoded_key() {
    use oss_reader::aggs::bucket::terms::TermsResult;
    use oss_reader::aggs::partial::{Format, Partial, Value as Part};
    use oss_transport::actions::search_shard::Order;
    let key = oss_reader::aggs::bucket::terms_collect::key_of_double(2.5);
    let part = Partial {
        name: "u".to_owned(),
        format: Format::Double,
        value: Part::Terms(Box::new(TermsResult {
            buckets: vec![(key, 3)],
            ..Default::default()
        })),
    };
    let answer = ask_with(vec![terms_request("u", Order::KeyAsc)], vec![part]).unwrap();
    let mut name = vec![1, 6];
    name.extend_from_slice(b"dterms");
    assert!(
        answer.windows(name.len()).any(|w| w == &name[..]),
        "{answer:?}"
    );
    let key = 2.5f64.to_be_bytes();
    assert!(answer.windows(8).any(|w| w == key), "{answer:?}");
}

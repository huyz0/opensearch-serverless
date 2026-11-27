//! The `rare_terms` and `multi_terms` aggregations of a forwarded search,
//! answered with the bytes OpenSearch 3.10.0 writes
//! (`fixtures/search-shard/java-rare-multi-3.10.0.txt`).

use super::aggregations::ask_with;
use oss_reader::aggs::bucket::multi::{MultiPart, MultiResult};
use oss_reader::aggs::bucket::rare::{RareKey, RareResult, RareSpec};
use oss_reader::aggs::bucket::terms::TermsSpec;
use oss_reader::aggs::partial::{Format, Partial, Value as Part};
use oss_transport::actions::search_shard::{
    AggKind, AggRequest, MultiRequest, Order, OrderSpec, RareRequest, TermsRequest,
};

const RECORDED: &str =
    include_str!("../../../../../fixtures/search-shard/java-rare-multi-3.10.0.txt");

/// The filter's start in an answer: its threshold, set mode, capacity and
/// false-positive rate.
const FILTER: [u8; 14] = [
    0x90, 0x4e, 0x01, 0xc0, 0x84, 0x3d, 0x3f, 0x50, 0x62, 0x4d, 0xd2, 0xf1, 0xa9, 0xfc,
];

fn recorded(name: &str) -> Vec<u8> {
    let hex = RECORDED
        .lines()
        .find_map(|l| l.strip_prefix(name).and_then(|r| r.strip_prefix(' ')))
        .unwrap_or_else(|| panic!("no fixture {name}"));
    hex.as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).expect("ascii"), 16).expect("hex"))
        .collect()
}

fn contains(answer: &[u8], bytes: &[u8]) -> bool {
    answer.windows(bytes.len()).any(|w| w == bytes)
}

fn base(name: &str, kind: AggKind, field: &str) -> AggRequest {
    AggRequest {
        name: name.to_owned(),
        kind,
        field: field.to_owned(),
        precision_threshold: None,
        percentiles: None,
        terms: None,
        histogram: None,
        ranges: None,
        rare: None,
        multi: None,
        subs: Vec::new(),
    }
}

fn rare_request(max_doc_count: i64) -> AggRequest {
    let mut a = base("r", AggKind::RareTerms, "tag");
    a.rare = Some(RareRequest { max_doc_count });
    a
}

fn text(t: &str) -> RareKey {
    RareKey::Text(t.to_owned())
}

fn rare_part(
    long: bool,
    max: u64,
    buckets: Vec<(RareKey, u64)>,
    excluded: Vec<RareKey>,
) -> Partial {
    Partial {
        name: "r".to_owned(),
        format: Format::Raw,
        value: Part::Rare(Box::new(RareResult {
            spec: RareSpec {
                max_doc_count: max,
                ..RareSpec::default()
            },
            long,
            buckets,
            excluded,
        })),
    }
}

#[test]
fn a_keyword_rare_terms_is_answered_as_string_rare_terms_with_its_filter() {
    let part = rare_part(
        false,
        1,
        vec![(text("a"), 1), (text("b"), 1)],
        vec![text("x"), text("y")],
    );
    let answer = ask_with(vec![rare_request(1)], vec![part]).unwrap();
    // The hashes of `x` and `y` are those Java computed for them.
    assert!(
        contains(&answer, &recorded("response.rare_keyword")),
        "{answer:?}"
    );
}

#[test]
fn a_number_rare_terms_is_answered_as_long_rare_terms_with_the_hashes_java_computed() {
    let part = rare_part(
        true,
        2,
        vec![(RareKey::Long(5), 1), (RareKey::Long(1), 2)],
        vec![RareKey::Long(7), RareKey::Long(-3)],
    );
    let answer = ask_with(vec![rare_request(2)], vec![part]).unwrap();
    // The recorded answer lists its buckets as built, the key 1 first; this
    // one lists them by count, so only the name and the filter are compared.
    let wire = recorded("response.rare_long");
    let at = wire
        .windows(FILTER.len())
        .position(|w| w == FILTER)
        .expect("a filter");
    assert!(contains(&answer, &wire[at..]), "{answer:?}");
    assert!(contains(&answer, b"lrareterms"));
}

#[test]
fn a_rare_terms_with_more_common_terms_than_its_set_holds_is_refused() {
    let excluded = (0..10_001).map(RareKey::Long).collect();
    let part = rare_part(true, 1, Vec::new(), excluded);
    let got = ask_with(vec![rare_request(1)], vec![part]);
    assert!(got.is_err());
}

fn multi_request() -> AggRequest {
    let mut a = base("m", AggKind::MultiTerms, "tag");
    a.multi = Some(MultiRequest {
        fields: vec!["tag".to_owned(), "n".to_owned()],
        options: TermsRequest {
            size: 10,
            shard_size: -1,
            min_doc_count: 1,
            shard_min_doc_count: 0,
            order: OrderSpec {
                compound: true,
                items: vec![Order::CountDesc, Order::KeyAsc],
            },
            show_error: false,
        },
    });
    a
}

fn multi_part(buckets: Vec<(&str, i64, u64)>, other: u64) -> Partial {
    Partial {
        name: "m".to_owned(),
        format: Format::Raw,
        value: Part::Multi(Box::new(MultiResult {
            spec: TermsSpec {
                size: 10,
                shard_size: 12,
                ..TermsSpec::default()
            },
            other_doc_count: other,
            doc_count_error: 0,
            buckets: buckets
                .into_iter()
                .map(|(t, n, c)| (vec![MultiPart::Text(t.to_owned()), MultiPart::Long(n)], c))
                .collect(),
            bucket_errors: Vec::new(),
            reduced: false,
        })),
    }
}

#[test]
fn a_multi_terms_that_shows_its_error_bound_writes_each_buckets() {
    let mut request = multi_request();
    request.multi.as_mut().unwrap().options.show_error = true;
    let mut part = multi_part(vec![("a", 1, 3), ("b", 2, 1)], 4);
    if let Part::Multi(m) = &mut part.value {
        m.spec.show_error = true;
        m.doc_count_error = 3;
        m.bucket_errors = vec![0, 7];
    }
    let answer = ask_with(vec![request], vec![part]).unwrap();
    assert!(
        contains(&answer, &recorded("response.multi_terms_errors")),
        "{answer:?}"
    );
}

#[test]
fn a_multi_terms_is_answered_as_the_bytes_java_wrote() {
    let part = multi_part(vec![("a", 1, 3), ("b", 2, 1)], 4);
    let answer = ask_with(vec![multi_request()], vec![part]).unwrap();
    assert!(
        contains(&answer, &recorded("response.multi_terms")),
        "{answer:?}"
    );
    let answer = ask_with(vec![multi_request()], vec![multi_part(Vec::new(), 0)]).unwrap();
    assert!(
        contains(&answer, &recorded("response.multi_terms_empty")),
        "{answer:?}"
    );
}

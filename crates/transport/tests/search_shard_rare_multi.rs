//! The `rare_terms` and `multi_terms` aggregations of a forwarded shard
//! search against the bytes OpenSearch 3.10.0 writes
//! (`fixtures/search-shard/java-rare-multi-3.10.0.txt`).

use oss_transport::actions::search_shard::*;
use oss_transport::stream::{Input, Output};

const RECORDED: &str = include_str!("../../../fixtures/search-shard/java-rare-multi-3.10.0.txt");

/// The filter's start in an answer: its threshold (10,000), set mode,
/// capacity (1,000,000) and false-positive rate (0.001).
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

/// The hashes the recorded filter holds, in the order Java wrote them: the
/// shard's answer holds them in the order it added them, so a test hands
/// them over in another one.
fn filter_hashes(wire: &[u8]) -> Vec<i64> {
    let at = wire
        .windows(FILTER.len())
        .position(|w| w == FILTER)
        .expect("a filter")
        .saturating_add(FILTER.len());
    let mut input = Input::new(&wire[at..]);
    let n = input.vint("count").expect("count");
    (0..n)
        .map(|_| input.zlong("hash").expect("a hash"))
        .collect()
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

fn rare(field: &str, max_doc_count: i64) -> AggRequest {
    let mut a = base("r", AggKind::RareTerms, field);
    a.rare = Some(RareRequest { max_doc_count });
    a
}

fn multi(options: TermsRequest) -> AggRequest {
    let mut a = base("m", AggKind::MultiTerms, "tag");
    a.multi = Some(MultiRequest {
        fields: vec!["tag".to_owned(), "n".to_owned()],
        options,
    });
    a
}

fn round_trips(wire: &[u8], expected: &[AggRequest]) {
    let mut input = Input::new(wire);
    assert_eq!(aggs::read(&mut input).expect("reads"), expected);
    input.finish("aggregations").expect("all read");
    let mut buf = Vec::new();
    aggs::write(expected, &mut Output::new(&mut buf, 1 << 20)).expect("writes");
    assert_eq!(buf, wire);
}

#[test]
fn the_rare_terms_builders_are_the_bytes_java_wrote() {
    round_trips(&recorded("request.rare_terms"), &[rare("tag", 1)]);
    // `precision` is not on the wire.
    round_trips(&recorded("request.rare_terms_options"), &[rare("n", 5)]);
}

#[test]
fn the_multi_terms_builders_are_the_bytes_java_wrote() {
    let default = TermsRequest {
        size: 10,
        shard_size: -1,
        min_doc_count: 1,
        shard_min_doc_count: 0,
        order: OrderSpec {
            compound: true,
            items: vec![Order::CountDesc, Order::KeyAsc],
        },
        show_error: false,
    };
    round_trips(&recorded("request.multi_terms"), &[multi(default)]);
    let options = TermsRequest {
        size: 3,
        shard_size: 7,
        min_doc_count: 2,
        shard_min_doc_count: 1,
        order: OrderSpec {
            compound: false,
            items: vec![Order::KeyDesc],
        },
        show_error: false,
    };
    round_trips(&recorded("request.multi_terms_options"), &[multi(options)]);
}

fn rare_written(a: RareAnswer) -> Vec<u8> {
    let mut buf = Vec::new();
    aggs::write_answers(
        &[AggAnswer {
            name: "r",
            value: AggValue::Rare(a),
            date: false,
        }],
        &mut Output::new(&mut buf, 1 << 20),
    )
    .expect("writes");
    buf
}

fn text(terms: &[&str], counts: &[i64], excluded: Vec<i64>) -> RareAnswer {
    RareAnswer {
        max_doc_count: 1,
        keys: RareKeys::Text(terms.iter().map(|t| (*t).to_owned()).collect()),
        counts: counts.to_vec(),
        excluded,
    }
}

/// Hands `hashes` back in the order a shard adds them, which is not Java's
/// listing order.
fn reversed(mut hashes: Vec<i64>) -> Vec<i64> {
    hashes.reverse();
    hashes
}

#[test]
fn a_rare_terms_answer_is_the_bytes_java_wrote() {
    let empty = text(&[], &[], Vec::new());
    assert_eq!(rare_written(empty), recorded("response.rare_keyword_empty"));
    let wire = recorded("response.rare_keyword");
    let two = text(&["a", "b"], &[1, 1], reversed(filter_hashes(&wire)));
    assert_eq!(rare_written(two), wire);
    let wire = recorded("response.rare_long");
    let longs = RareAnswer {
        max_doc_count: 2,
        keys: RareKeys::Long(vec![1, 5]),
        counts: vec![2, 1],
        excluded: reversed(filter_hashes(&wire)),
    };
    assert_eq!(rare_written(longs), wire);
}

#[test]
fn the_filter_lists_its_hashes_in_the_order_of_a_java_hash_set() {
    // Thirty and twenty-five hashes, some in one bucket of the set's table.
    for name in ["response.rare_keyword_many", "response.rare_long_many"] {
        let wire = recorded(name);
        let hashes = filter_hashes(&wire);
        assert!(hashes.len() >= 25, "{name}");
        let long = name.contains("long");
        let answer = RareAnswer {
            max_doc_count: 1,
            keys: if long {
                RareKeys::Long(Vec::new())
            } else {
                RareKeys::Text(Vec::new())
            },
            counts: Vec::new(),
            excluded: hashes.clone(),
        };
        assert_eq!(rare_written(answer), wire, "{name}");
    }
}

#[test]
fn a_filter_past_its_set_threshold_is_refused() {
    let many = text(&[], &[], (0..10_001).collect());
    let mut buf = Vec::new();
    let got = aggs::write_answers(
        &[AggAnswer {
            name: "r",
            value: AggValue::Rare(many),
            date: false,
        }],
        &mut Output::new(&mut buf, 1 << 20),
    );
    assert!(got.is_err());
}

fn multi_written(a: MultiAnswer) -> Vec<u8> {
    let mut buf = Vec::new();
    aggs::write_answers(
        &[AggAnswer {
            name: "m",
            value: AggValue::Multi(a),
            date: false,
        }],
        &mut Output::new(&mut buf, 1 << 20),
    )
    .expect("writes");
    buf
}

fn multi_answer(keys: Vec<Vec<MultiTerm>>, counts: Vec<i64>, other: i64) -> MultiAnswer {
    MultiAnswer {
        order: OrderSpec {
            compound: true,
            items: vec![Order::CountDesc, Order::KeyAsc],
        },
        size: 10,
        min_doc_count: 1,
        doc_count_error: 0,
        shard_size: 12,
        other_doc_count: other,
        terms: 2,
        keys,
        counts,
    }
}

#[test]
fn a_multi_terms_answer_is_the_bytes_java_wrote() {
    let key = |t: &str, n: i64| vec![MultiTerm::Text(t.to_owned()), MultiTerm::Long(n)];
    let two = multi_answer(vec![key("a", 1), key("b", 2)], vec![3, 1], 4);
    assert_eq!(multi_written(two), recorded("response.multi_terms"));
    let none = multi_answer(Vec::new(), Vec::new(), 0);
    assert_eq!(multi_written(none), recorded("response.multi_terms_empty"));
}

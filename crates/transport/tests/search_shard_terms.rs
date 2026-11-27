//! The `terms` aggregation of a forwarded shard search against the bytes
//! OpenSearch 3.10.0 writes (`fixtures/search-shard/java-terms-3.10.0.txt`).

use oss_transport::actions::search_shard::*;
use oss_transport::stream::{Input, Output};

const RECORDED: &str = include_str!("../../../fixtures/search-shard/java-terms-3.10.0.txt");

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

fn terms(name: &str, field: &str, t: TermsRequest) -> AggRequest {
    AggRequest {
        name: name.to_owned(),
        kind: AggKind::Terms,
        field: field.to_owned(),
        precision_threshold: None,
        percentiles: None,
        terms: Some(t),
    }
}

fn written(answer: AggAnswer<'_>) -> Vec<u8> {
    let mut buf = Vec::new();
    aggs::write_answers(&[answer], &mut Output::new(&mut buf, 1 << 20)).expect("writes");
    buf
}

#[test]
fn the_terms_builders_are_the_bytes_java_wrote() {
    let wire = recorded("request.terms");
    let expected = vec![
        terms(
            "t",
            "tag",
            TermsRequest {
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
        ),
        terms(
            "u",
            "n",
            TermsRequest {
                size: 5,
                shard_size: 20,
                min_doc_count: 2,
                shard_min_doc_count: 0,
                order: OrderSpec {
                    compound: false,
                    items: vec![Order::KeyAsc],
                },
                show_error: true,
            },
        ),
    ];
    let mut input = Input::new(&wire);
    assert_eq!(aggs::read(&mut input).expect("reads"), expected);
    input.finish("aggregations").expect("all read");
    let mut buf = Vec::new();
    aggs::write(&expected, &mut Output::new(&mut buf, 1 << 20)).expect("writes");
    assert_eq!(buf, wire);
}

#[test]
fn a_terms_with_a_sub_aggregation_is_refused_by_name() {
    assert!(matches!(
        aggs::read(&mut Input::new(&recorded("request.terms_subs"))),
        Err(Refusal::Unsupported("sub-aggregations"))
    ));
}

fn answer(reduce_order: Order, order: Order, keys: Keys, counts: Vec<i64>) -> TermsAnswer {
    TermsAnswer {
        reduce_order,
        order: OrderSpec {
            compound: false,
            items: vec![order],
        },
        size: 10,
        min_doc_count: 1,
        doc_count_error: 0,
        shard_size: 50,
        show_error: false,
        other_doc_count: 0,
        keys,
        counts,
        errors: Vec::new(),
    }
}

#[test]
fn a_string_terms_answer_is_the_bytes_java_wrote() {
    let mut a = answer(
        Order::CountDesc,
        Order::CountDesc,
        Keys::Text(vec!["a".to_owned(), "b".to_owned()]),
        vec![3, 1],
    );
    a.other_doc_count = 2;
    let got = written(AggAnswer {
        name: "t",
        value: AggValue::Terms(a.clone()),
        date: false,
    });
    assert_eq!(got, recorded("response.string_terms"));
    a.other_doc_count = 0;
    a.keys = Keys::Text(Vec::new());
    a.counts = Vec::new();
    let got = written(AggAnswer {
        name: "t",
        value: AggValue::Terms(a),
        date: false,
    });
    assert_eq!(got, recorded("response.string_terms_empty"));
}

#[test]
fn a_long_terms_answer_is_the_bytes_java_wrote() {
    let mut a = answer(Order::KeyAsc, Order::KeyAsc, Keys::Long(vec![7]), vec![3]);
    a.doc_count_error = -1;
    a.shard_size = 20;
    a.show_error = true;
    a.errors = vec![1];
    let got = written(AggAnswer {
        name: "u",
        value: AggValue::Terms(a),
        date: false,
    });
    assert_eq!(got, recorded("response.long_terms"));
}

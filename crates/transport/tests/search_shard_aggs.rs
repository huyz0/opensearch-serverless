//! The aggregations of a forwarded shard search against the bytes OpenSearch
//! 3.10.0 writes (`fixtures/search-shard/java-aggs-3.10.0.txt` and
//! `java-cardinality-3.10.0.txt`).

use oss_transport::actions::search_shard::*;
use oss_transport::stream::Output;

const AGGS: &str = include_str!("../../../fixtures/search-shard/java-aggs-3.10.0.txt");

fn recorded(name: &str) -> Vec<u8> {
    let hex = AGGS
        .lines()
        .find_map(|l| l.strip_prefix(name).and_then(|r| r.strip_prefix(' ')))
        .unwrap_or_else(|| panic!("no fixture {name}"));
    hex.as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).expect("ascii"), 16).expect("hex"))
        .collect()
}

fn request(name: &str, kind: AggKind, field: &str) -> AggRequest {
    AggRequest {
        name: name.to_owned(),
        kind,
        field: field.to_owned(),
        precision_threshold: None,
    }
}

fn answer(name: &str, value: AggValue, date: bool) -> AggAnswer<'_> {
    AggAnswer { name, value, date }
}

fn written(answers: &[AggAnswer<'_>]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut out = Output::new(&mut buf, 1 << 20);
    aggs::write_answers(answers, &mut out).expect("writes");
    buf
}

#[test]
fn the_metric_builders_are_read_and_written_as_java_wrote_them() {
    let wire = recorded("request.metrics");
    let expected = vec![
        request("hi", AggKind::Max, "n"),
        request("c", AggKind::ValueCount, "tag"),
        request("lo", AggKind::Min, "n"),
        request("s", AggKind::Sum, "n"),
        request("av", AggKind::Avg, "n"),
        request("st", AggKind::Stats, "n"),
    ];
    let mut input = oss_transport::stream::Input::new(&wire);
    assert_eq!(aggs::read(&mut input).expect("reads"), expected);
    input.finish("aggregations").expect("all read");
    let mut buf = Vec::new();
    aggs::write(&expected, &mut Output::new(&mut buf, 1 << 20)).expect("writes");
    assert_eq!(buf, wire);
}

#[test]
fn the_metric_answers_are_the_bytes_java_wrote() {
    let got = written(&[
        answer("hi", AggValue::Max(2.0), false),
        answer("c", AggValue::ValueCount(7), false),
        answer("lo", AggValue::Min(1.0), false),
        answer("s", AggValue::Sum(3.5), false),
        answer("av", AggValue::Avg { sum: 6.0, count: 3 }, false),
        answer(
            "st",
            AggValue::Stats {
                count: 3,
                min: 1.0,
                max: 4.0,
                sum: 6.0,
            },
            false,
        ),
    ]);
    assert_eq!(got, recorded("response.metrics"));
}

#[test]
fn a_date_answer_is_the_bytes_java_wrote() {
    let got = written(&[answer("t", AggValue::Max(1.0), true)]);
    assert_eq!(got, recorded("response.date_max"));
}

const CARDINALITY: &str =
    include_str!("../../../fixtures/search-shard/java-cardinality-3.10.0.txt");

fn card(name: &str) -> Vec<u8> {
    let hex = CARDINALITY
        .lines()
        .find_map(|l| l.strip_prefix(name).and_then(|r| r.strip_prefix(' ')))
        .unwrap_or_else(|| panic!("no fixture {name}"));
    hex.as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).expect("ascii"), 16).expect("hex"))
        .collect()
}

#[test]
fn the_cardinality_builders_are_the_bytes_java_wrote() {
    let wire = card("request.cardinality");
    let mut u = request("u", AggKind::Cardinality, "tag");
    let mut v = request("v", AggKind::Cardinality, "n");
    v.precision_threshold = Some(100);
    u.precision_threshold = None;
    let expected = vec![u, v];
    let mut input = oss_transport::stream::Input::new(&wire);
    assert_eq!(aggs::read(&mut input).expect("reads"), expected);
    input.finish("aggregations").expect("all read");
    let mut buf = Vec::new();
    aggs::write(&expected, &mut Output::new(&mut buf, 1 << 20)).expect("writes");
    assert_eq!(buf, wire);
}

#[test]
fn a_cardinality_answer_is_the_bytes_java_wrote_around_the_sketch() {
    let prefix_len = "010b63617264696e616c6974790175ff03726177".len() / 2;
    assert_eq!(
        written(&[answer("u", AggValue::Cardinality(None), false)])[prefix_len..],
        [0]
    );
    for name in [
        "response.cardinality_empty",
        "response.cardinality_linear",
        "response.cardinality_hll",
    ] {
        let java = card(name);
        let sketch = java[prefix_len + 1..].to_vec();
        assert_eq!(java[prefix_len], 1, "{name}");
        let got = written(&[answer("u", AggValue::Cardinality(Some(sketch)), false)]);
        assert_eq!(got, java, "{name}");
    }
}

#[test]
fn an_execution_hint_is_refused() {
    let mut wire = Vec::new();
    {
        let mut out = Output::new(&mut wire, 1 << 20);
        let mut a = request("u", AggKind::Cardinality, "tag");
        a.precision_threshold = None;
        aggs::write(&[a], &mut out).expect("writes");
    }
    // Replace the execution hint's `false` (the byte before the pipeline
    // count) with `true` and a one-letter hint.
    let last = wire.len() - 2;
    wire[last] = 1;
    wire.splice(last + 1..last + 1, [1, b'x']);
    assert!(matches!(
        aggs::read(&mut oss_transport::stream::Input::new(&wire)),
        Err(Refusal::Unsupported("an execution hint"))
    ));
}

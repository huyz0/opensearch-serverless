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
        percentiles: None,
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

const PERCENTILES: &str =
    include_str!("../../../fixtures/search-shard/java-percentiles-3.10.0.txt");

fn pct(name: &str) -> Vec<u8> {
    let hex = PERCENTILES
        .lines()
        .find_map(|l| l.strip_prefix(name).and_then(|r| r.strip_prefix(' ')))
        .unwrap_or_else(|| panic!("no fixture {name}"));
    hex.as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).expect("ascii"), 16).expect("hex"))
        .collect()
}

fn percentiles(name: &str, kind: AggKind, keys: &[f64], keyed: bool, c: Option<f64>) -> AggRequest {
    let mut r = request(name, kind, "n");
    r.percentiles = Some(PercentilesRequest {
        keys: keys.to_vec(),
        keyed,
        compression: c,
    });
    r
}

#[test]
fn the_percentiles_builders_are_the_bytes_java_wrote() {
    let wire = pct("request.percentiles");
    let expected = vec![
        percentiles(
            "p",
            AggKind::Percentiles,
            &[1.0, 5.0, 25.0, 50.0, 75.0, 95.0, 99.0],
            true,
            None,
        ),
        percentiles("q", AggKind::Percentiles, &[50.0, 99.5], false, Some(200.0)),
        percentiles("r", AggKind::PercentileRanks, &[10.0, 20.0], true, None),
    ];
    let mut input = oss_transport::stream::Input::new(&wire);
    assert_eq!(aggs::read(&mut input).expect("reads"), expected);
    input.finish("aggregations").expect("all read");
    let mut buf = Vec::new();
    aggs::write(&expected, &mut Output::new(&mut buf, 1 << 20)).expect("writes");
    assert_eq!(buf, wire);
}

/// The type name, the aggregation's name, no metadata and the format `raw`.
fn answer_parts(java: &[u8]) -> (Vec<f64>, Vec<u8>, bool) {
    let mut input = oss_transport::stream::Input::new(java);
    assert_eq!(input.vint("n").expect("one"), 1);
    input.string("type").expect("type");
    input.string("name").expect("name");
    assert_eq!(input.byte("metadata").expect("byte"), 0xFF);
    assert_eq!(input.string("format").expect("format"), "raw");
    let n = input.count(8, "keys").expect("keys");
    let keys = (0..n)
        .map(|_| f64::from_bits(input.long("key").expect("key").cast_unsigned()))
        .collect();
    let digest = input.bytes_reference("digest").expect("digest").to_vec();
    let keyed = input.bool("keyed").expect("keyed");
    input.finish("answer").expect("all read");
    (keys, digest, keyed)
}

#[test]
fn a_percentiles_answer_is_the_bytes_java_wrote_around_the_digest() {
    for (name, ranks) in [
        ("response.percentiles_small", false),
        ("response.percentiles_big", false),
        ("response.percentiles_empty", false),
        ("response.ranks", true),
    ] {
        let java = pct(name);
        let (keys, digest, keyed) = answer_parts(&java);
        let got = written(&[answer(
            if name.ends_with("big") {
                "q"
            } else if ranks {
                "r"
            } else {
                "p"
            },
            AggValue::Percentiles {
                ranks,
                keys,
                digest,
                keyed,
            },
            false,
        )]);
        assert_eq!(got, java, "{name}");
    }
}

#[test]
fn a_percentiles_method_other_than_tdigest_is_refused() {
    let mut wire = pct("request.percentiles");
    // The first aggregation's config is absent; make the second's method 1.
    let at = wire
        .windows(9)
        .position(|w| w == [0, 1, 0, 0x40, 0x69, 0, 0, 0, 0])
        .expect("the second config");
    wire[at + 2] = 1;
    assert!(matches!(
        aggs::read(&mut oss_transport::stream::Input::new(&wire)),
        Err(Refusal::Unsupported(
            "a percentiles method other than tdigest"
        ))
    ));
}

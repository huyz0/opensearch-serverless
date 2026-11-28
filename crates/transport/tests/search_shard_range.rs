//! The `range` and `date_range` aggregations of a forwarded shard search
//! against the bytes OpenSearch 3.10.0 writes
//! (`fixtures/search-shard/java-range-3.10.0.txt`).

use oss_transport::actions::search_shard::*;
use oss_transport::stream::{Input, Output};

const RECORDED: &str = include_str!("../../../fixtures/search-shard/java-range-3.10.0.txt");
const NEG: f64 = f64::NEG_INFINITY;
const POS: f64 = f64::INFINITY;

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

fn ask(key: Option<&str>, from: f64, to: f64) -> RangeAsk {
    RangeAsk {
        key: key.map(str::to_owned),
        from,
        from_text: None,
        to,
        to_text: None,
    }
}

fn request(
    name: &str,
    kind: AggKind,
    field: &str,
    ranges: Vec<RangeAsk>,
    keyed: bool,
) -> AggRequest {
    AggRequest {
        name: name.to_owned(),
        kind,
        field: field.to_owned(),
        precision_threshold: None,
        percentiles: None,
        terms: None,
        histogram: None,
        ranges: Some(RangesRequest { ranges, keyed }),
        rare: None,
        multi: None,
        wrap_longitude: true,
        subs: Vec::new(),
    }
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
fn the_range_builders_are_the_bytes_java_wrote() {
    let three = vec![
        ask(Some("low"), NEG, 10.0),
        ask(None, 10.0, 20.0),
        ask(None, 20.0, POS),
    ];
    round_trips(
        &recorded("request.range"),
        &[request("r", AggKind::Range, "n", three, false)],
    );
    round_trips(
        &recorded("request.range_keyed"),
        &[request(
            "r",
            AggKind::Range,
            "n",
            vec![ask(Some("mid"), 5.0, 15.0)],
            true,
        )],
    );
    let mut with_sub = request("r", AggKind::Range, "n", vec![ask(None, 0.0, 10.0)], false);
    with_sub.subs = vec![AggRequest {
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
    round_trips(&recorded("request.range_subs"), &[with_sub]);
}

#[test]
fn the_date_range_builders_are_the_bytes_java_wrote() {
    round_trips(
        &recorded("request.date_range"),
        &[request(
            "d",
            AggKind::DateRange,
            "t",
            vec![ask(Some("old"), NEG, 1000.0), ask(None, 1000.0, 2000.0)],
            false,
        )],
    );
    let text = RangeAsk {
        key: Some("2020".to_owned()),
        from: NEG,
        from_text: Some("2020-01-01".to_owned()),
        to: POS,
        to_text: Some("2021-01-01".to_owned()),
    };
    round_trips(
        &recorded("request.date_range_text"),
        &[request("d", AggKind::DateRange, "t", vec![text], true)],
    );
}

fn bucket(key: &str, from: f64, to: f64, count: i64) -> RangeBucketAnswer {
    RangeBucketAnswer {
        key: key.to_owned(),
        from,
        to,
        count,
        subs: Vec::new(),
    }
}

fn written(name: &str, date: bool, a: RangeAnswer) -> Vec<u8> {
    let mut buf = Vec::new();
    aggs::write_answers(
        &[AggAnswer {
            name,
            value: AggValue::Range(a),
            date,
        }],
        &mut Output::new(&mut buf, 1 << 20),
    )
    .expect("writes");
    buf
}

#[test]
fn a_range_answer_is_the_bytes_java_wrote() {
    for (keyed, name) in [(false, "response.range"), (true, "response.range_keyed")] {
        let a = RangeAnswer {
            date_range: false,
            keyed,
            buckets: vec![
                bucket("low", NEG, 10.0, 3),
                bucket("10.0-20.0", 10.0, 20.0, 0),
                bucket("20.0-*", 20.0, POS, 5),
            ],
        };
        assert_eq!(written("r", false, a), recorded(name), "{name}");
    }
    let mut with_subs = bucket("0.0-10.0", 0.0, 10.0, 3);
    with_subs.subs = vec![SubAnswer {
        name: "m".to_owned(),
        value: AggValue::Max(4.0),
        date: false,
    }];
    let a = RangeAnswer {
        date_range: false,
        keyed: false,
        buckets: vec![with_subs],
    };
    assert_eq!(written("r", false, a), recorded("response.range_subs"));
    let none = RangeAnswer {
        date_range: false,
        keyed: false,
        buckets: Vec::new(),
    };
    assert_eq!(written("r", false, none), recorded("response.range_empty"));
}

#[test]
fn a_date_range_answer_is_the_bytes_java_wrote() {
    let one = "1970-01-01T00:00:01.000Z";
    let two = "1970-01-01T00:00:02.000Z";
    for (keyed, name) in [
        (false, "response.date_range"),
        (true, "response.date_range_keyed"),
    ] {
        let a = RangeAnswer {
            date_range: true,
            keyed,
            buckets: vec![
                bucket("old", NEG, 1000.0, 3),
                bucket(&format!("{one}-{two}"), 1000.0, 2000.0, 2),
                bucket(&format!("{two}-*"), 2000.0, POS, 1),
            ],
        };
        assert_eq!(written("d", true, a), recorded(name), "{name}");
    }
}

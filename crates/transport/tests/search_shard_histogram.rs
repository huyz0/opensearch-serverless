//! The `date_histogram` aggregation of a forwarded shard search against the
//! bytes OpenSearch 3.10.0 writes
//! (`fixtures/search-shard/java-date-histogram-3.10.0.txt`).

use oss_transport::actions::search_shard::*;
use oss_transport::stream::{Input, Output};

const RECORDED: &str =
    include_str!("../../../fixtures/search-shard/java-date-histogram-3.10.0.txt");

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

fn histogram(name: &str, h: DateHistogramRequest) -> AggRequest {
    AggRequest {
        name: name.to_owned(),
        kind: AggKind::DateHistogram,
        field: "t".to_owned(),
        precision_threshold: None,
        percentiles: None,
        terms: None,
        histogram: Some(h),
        ranges: None,
        rare: None,
        multi: None,
        wrap_longitude: true,
        subs: Vec::new(),
    }
}

fn day() -> DateHistogramRequest {
    DateHistogramRequest {
        order: OrderSpec {
            compound: false,
            items: vec![Order::KeyAsc],
        },
        keyed: false,
        min_doc_count: 0,
        interval: "1d".to_owned(),
        interval_type: IntervalType::Calendar,
        offset: 0,
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
fn the_date_histogram_builders_are_the_bytes_java_wrote() {
    let fixed = DateHistogramRequest {
        order: OrderSpec {
            compound: false,
            items: vec![Order::KeyDesc],
        },
        keyed: false,
        min_doc_count: 1,
        interval: "30s".to_owned(),
        interval_type: IntervalType::Fixed,
        offset: 5000,
    };
    round_trips(
        &recorded("request.date_histogram"),
        &[histogram("d", day()), histogram("e", fixed)],
    );
    let mut with_sub = histogram("d", day());
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
    round_trips(&recorded("request.date_histogram_subs"), &[with_sub]);
}

fn max(v: f64) -> Vec<SubAnswer> {
    vec![SubAnswer {
        name: "m".to_owned(),
        value: AggValue::Max(v),
        date: false,
    }]
}

fn written(name: &str, a: DateHistogramAnswer) -> Vec<u8> {
    let mut buf = Vec::new();
    aggs::write_answers(
        &[AggAnswer {
            name,
            value: AggValue::DateHistogram(a),
            date: true,
        }],
        &mut Output::new(&mut buf, 1 << 20),
    )
    .expect("writes");
    buf
}

fn answer(rounding: RoundingWire, keys: Vec<i64>, counts: Vec<i64>) -> DateHistogramAnswer {
    DateHistogramAnswer {
        order: day().order,
        min_doc_count: 0,
        rounding,
        empty_subs: Vec::new(),
        offset: 0,
        keyed: false,
        keys,
        counts,
        subs: Vec::new(),
    }
}

#[test]
fn a_date_histogram_answer_is_the_bytes_java_wrote() {
    let two = || {
        answer(
            RoundingWire::Unit(5),
            vec![86_400_000, 172_800_000],
            vec![3, 1],
        )
    };
    assert_eq!(written("d", two()), recorded("response.date_histogram"));
    let mut with_subs = two();
    with_subs.empty_subs = max(f64::NEG_INFINITY);
    with_subs.subs = vec![max(4.0), Vec::new()];
    assert_eq!(
        written("d", with_subs),
        recorded("response.date_histogram_subs")
    );
    let fixed = answer(RoundingWire::Interval(30_000), vec![30_000], vec![2]);
    assert_eq!(
        written("d", fixed),
        recorded("response.date_histogram_fixed")
    );
}

#[test]
fn a_minimum_count_above_zero_writes_no_empty_bucket_info_and_the_offset() {
    let mut a = answer(RoundingWire::Interval(30_000), vec![30_000], vec![2]);
    a.order = OrderSpec {
        compound: false,
        items: vec![Order::KeyDesc],
    };
    a.min_doc_count = 1;
    a.offset = 5000;
    assert_eq!(written("e", a), recorded("response.date_histogram_min1"));
}

#[test]
fn every_calendar_unit_has_the_id_java_writes() {
    for (unit, id) in [
        ("week_of_weekyear", 1),
        ("year_of_century", 2),
        ("quarter_of_year", 3),
        ("month_of_year", 4),
        ("day_of_month", 5),
        ("hour_of_day", 6),
        ("minutes_of_hour", 7),
        ("second_of_minute", 8),
    ] {
        let got = written("d", answer(RoundingWire::Unit(id), Vec::new(), Vec::new()));
        assert_eq!(
            got,
            recorded(&format!("response.rounding_{unit}")),
            "{unit}"
        );
    }
}

#[test]
fn extended_bounds_and_a_legacy_interval_are_refused_by_name() {
    let mut wire = recorded("request.date_histogram_subs");
    // The two bounds are the last bytes before the pipeline count; turn the
    // extended bounds' `false` into `true`.
    let at = wire.len() - 3;
    wire[at] = 1;
    assert!(matches!(
        aggs::read(&mut Input::new(&wire)),
        Err(Refusal::Unsupported("extended bounds"))
    ));
    let mut wire = recorded("request.date_histogram_subs");
    // The interval type's `2` (calendar), after the string "1d", becomes 3.
    let at = wire
        .windows(3)
        .position(|w| w == [0x02, b'1', b'd'])
        .expect("interval")
        + 3;
    wire[at] = 3;
    assert!(matches!(
        aggs::read(&mut Input::new(&wire)),
        Err(Refusal::Unsupported("a legacy interval"))
    ));
}

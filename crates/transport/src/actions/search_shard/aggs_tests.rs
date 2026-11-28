//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::*;

fn bytes(aggs: &[AggRequest]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut out = Output::new(&mut buf, 1 << 20);
    write(aggs, &mut out).expect("writes");
    buf
}

#[test]
fn a_written_request_reads_back() {
    let aggs = vec![
        AggRequest {
            name: "hi".into(),
            kind: AggKind::Max,
            field: "n".into(),
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
            name: "c".into(),
            kind: AggKind::ValueCount,
            field: "tag".into(),
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
    let wire = bytes(&aggs);
    let mut input = Input::new(&wire);
    assert_eq!(read(&mut input).expect("reads"), aggs);
    input.finish("aggregations").expect("all read");
}

#[test]
fn another_type_and_a_sub_aggregation_are_refused() {
    let mut wire = Vec::new();
    let mut out = Output::new(&mut wire, 1 << 20);
    out.vint(1).expect("count");
    out.string("geohash_grid").expect("type");
    assert!(matches!(
        read(&mut Input::new(&wire)),
        Err(Refusal::Unsupported("an aggregation of another type"))
    ));
    // A metric with a sub-aggregation, written as a builder would write it.
    let inner = AggRequest {
        name: "x".into(),
        kind: AggKind::Min,
        field: "n".into(),
        precision_threshold: None,
        percentiles: None,
        terms: None,
        histogram: None,
        ranges: None,
        rare: None,
        multi: None,
        wrap_longitude: true,
        subs: Vec::new(),
    };
    let outer = AggRequest {
        name: "m".into(),
        kind: AggKind::Max,
        field: "n".into(),
        precision_threshold: None,
        percentiles: None,
        terms: None,
        histogram: None,
        ranges: None,
        rare: None,
        multi: None,
        wrap_longitude: true,
        subs: vec![inner],
    };
    let wire = bytes(&[outer]);
    assert!(matches!(
        read(&mut Input::new(&wire)),
        Err(Refusal::Unsupported("sub-aggregations"))
    ));
}

#[test]
fn a_reply_is_a_list_of_named_writeables() {
    let mut buf = Vec::new();
    let mut out = Output::new(&mut buf, 1 << 20);
    let answers = [
        AggAnswer {
            name: "m",
            value: AggValue::Max(2.0),
            date: false,
        },
        AggAnswer {
            name: "c",
            value: AggValue::ValueCount(7),
            date: false,
        },
    ];
    write_answers(&answers, &mut out).expect("writes");
    let mut expected = vec![2, 3, b'm', b'a', b'x', 1, b'm', 0xFF, 3, b'r', b'a', b'w'];
    expected.extend_from_slice(&2.0f64.to_bits().to_be_bytes());
    expected.extend_from_slice(&[11]);
    expected.extend_from_slice(b"value_count");
    expected.extend_from_slice(&[1, b'c', 0xFF, 7]);
    assert_eq!(buf, expected);
}

#[test]
fn a_date_field_is_answered_with_the_date_time_format() {
    let mut buf = Vec::new();
    let mut out = Output::new(&mut buf, 1 << 20);
    let answers = [AggAnswer {
        name: "t",
        value: AggValue::Max(1.0),
        date: true,
    }];
    write_answers(&answers, &mut out).expect("writes");
    let mut expected = vec![1, 3, b'm', b'a', b'x', 1, b't', 0xFF, 9];
    expected.extend_from_slice(b"date_time");
    let pattern = "strict_date_time_no_millis||strict_date_optional_time||epoch_millis";
    expected.push(u8::try_from(pattern.len()).unwrap());
    expected.extend_from_slice(pattern.as_bytes());
    expected.push(1);
    expected.push(25);
    expected.extend_from_slice(b"strict_date_optional_time");
    expected.extend_from_slice(&[1, b'Z', 0]);
    expected.extend_from_slice(&1.0f64.to_bits().to_be_bytes());
    assert_eq!(buf, expected);
}

fn geo(kind: AggKind, wrap_longitude: bool) -> AggRequest {
    AggRequest {
        name: "g".into(),
        kind,
        field: "loc".into(),
        precision_threshold: None,
        percentiles: None,
        terms: None,
        histogram: None,
        ranges: None,
        rare: None,
        multi: None,
        wrap_longitude,
        subs: Vec::new(),
    }
}

#[test]
fn a_geo_request_reads_back_with_its_wrap_flag() {
    let aggs = vec![
        geo(AggKind::GeoCentroid, true),
        geo(AggKind::GeoBounds, false),
    ];
    let wire = bytes(&aggs);
    let mut input = Input::new(&wire);
    assert_eq!(read(&mut input).expect("reads"), aggs);
    input.finish("aggregations").expect("all read");
}

fn answer(value: AggValue) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut out = Output::new(&mut buf, 1 << 20);
    write_one("g", &value, false, &mut out).expect("writes");
    buf
}

#[test]
fn a_centroid_answer_is_its_count_then_the_point_as_the_java_streams_it() {
    let wire = answer(AggValue::GeoCentroid {
        count: 3,
        lat: 20.0,
        lon: 30.0,
    });
    let mut want = Vec::new();
    let mut out = Output::new(&mut want, 1 << 20);
    out.string("geo_centroid").unwrap();
    out.string("g").unwrap();
    out.byte(0xFF).unwrap();
    out.vlong(3).unwrap();
    out.bool(true).unwrap();
    out.bytes(&20.0f64.to_bits().to_be_bytes()).unwrap();
    out.bytes(&30.0f64.to_bits().to_be_bytes()).unwrap();
    assert_eq!(wire, want);
    // No point: the count and a false flag only.
    let empty = answer(AggValue::GeoCentroid {
        count: 0,
        lat: 0.0,
        lon: 0.0,
    });
    assert_eq!(&empty[empty.len() - 2..], &[0, 0]);
}

#[test]
fn a_bounds_answer_is_six_doubles_then_the_wrap_flag() {
    let wire = answer(AggValue::GeoBounds {
        top: 1.0,
        bottom: 2.0,
        pos_left: 3.0,
        pos_right: 4.0,
        neg_left: 5.0,
        neg_right: 6.0,
        wrap: false,
    });
    let mut want = Vec::new();
    let mut out = Output::new(&mut want, 1 << 20);
    out.string("geo_bounds").unwrap();
    out.string("g").unwrap();
    out.byte(0xFF).unwrap();
    for v in [1.0f64, 2.0, 3.0, 4.0, 5.0, 6.0] {
        out.bytes(&v.to_bits().to_be_bytes()).unwrap();
    }
    out.bool(false).unwrap();
    assert_eq!(wire, want);
}

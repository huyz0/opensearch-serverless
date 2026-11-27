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
        },
        AggRequest {
            name: "c".into(),
            kind: AggKind::ValueCount,
            field: "tag".into(),
            precision_threshold: None,
            percentiles: None,
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
    out.string("terms").expect("type");
    assert!(matches!(
        read(&mut Input::new(&wire)),
        Err(Refusal::Unsupported("an aggregation of another type"))
    ));
    let mut wire = Vec::new();
    let mut out = Output::new(&mut wire, 1 << 20);
    out.vint(1).expect("count");
    out.string("max").expect("type");
    out.string("m").expect("name");
    out.vint(1).expect("a sub-aggregation");
    out.vint(0).expect("no pipelines");
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

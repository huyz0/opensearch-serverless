//! `half_float` and `scaled_float`: the mapping parameters and the values a
//! document gives them, as `NumberFieldMapper.NumberType.HALF_FLOAT` and
//! `ScaledFloatFieldMapper` (OpenSearch 3.10) read them.

use oss_doc::document::{self, FieldValue};
use oss_doc::mapping::{Mapping, MappingError, Num};

fn mapping(json: &str) -> Mapping {
    Mapping::parse(json.as_bytes()).unwrap()
}

fn values(m: &Mapping, doc: &str) -> Vec<FieldValue> {
    document::parse(m, "1", doc.as_bytes())
        .unwrap()
        .fields
        .into_iter()
        .map(|(_, v)| v)
        .collect()
}

fn cause(m: &Mapping, doc: &str) -> String {
    let e = document::parse(m, "1", doc.as_bytes()).unwrap_err();
    format!("{e:?}")
}

fn parse_error(json: &str) -> MappingError {
    Mapping::parse(json.as_bytes()).unwrap_err()
}

const M: &str = r#"{"properties":{
  "h":{"type":"half_float"},
  "s":{"type":"scaled_float","scaling_factor":100},
  "n":{"type":"scaled_float","scaling_factor":10,"null_value":1.5}}}"#;

#[test]
fn a_half_float_keeps_the_float_it_was_given() {
    let m = mapping(M);
    assert_eq!(
        values(&m, r#"{"h":1.1}"#),
        vec![FieldValue::Number(Num::Float(1.1))]
    );
    assert_eq!(
        values(&m, r#"{"h":"2.5"}"#),
        vec![FieldValue::Number(Num::Float(2.5))]
    );
}

#[test]
fn a_half_float_past_the_half_range_is_refused() {
    let m = mapping(M);
    assert!(
        cause(&m, r#"{"h":70000}"#)
            .contains("[half_float] supports only finite values, but got [70000.0]")
    );
    // 65504 is the largest half; the float just above it rounds to infinity.
    assert_eq!(
        values(&m, r#"{"h":65504}"#),
        vec![FieldValue::Number(Num::Float(65504.0))]
    );
}

#[test]
fn a_scaled_float_is_the_rounded_long_of_the_scaled_value() {
    let m = mapping(M);
    // `Math.round(1.234 * 100)`, ties up: 0.125 * 100 is 12.5, so 13, and -0.125 gives -12.
    assert_eq!(
        values(&m, r#"{"s":1.234}"#),
        vec![FieldValue::Number(Num::Long(123))]
    );
    assert_eq!(
        values(&m, r#"{"s":0.125}"#),
        vec![FieldValue::Number(Num::Long(13))]
    );
    assert_eq!(
        values(&m, r#"{"s":-0.125}"#),
        vec![FieldValue::Number(Num::Long(-12))]
    );
    assert_eq!(
        values(&m, r#"{"s":"3"}"#),
        vec![FieldValue::Number(Num::Long(300))]
    );
}

#[test]
fn a_scaled_float_null_value_is_scaled_too() {
    let m = mapping(M);
    assert_eq!(
        values(&m, r#"{"n":null}"#),
        vec![FieldValue::Number(Num::Long(15))]
    );
}

#[test]
fn a_scaled_float_refuses_what_it_cannot_carry() {
    let m = mapping(M);
    assert!(
        cause(&m, r#"{"s":"NaN"}"#)
            .contains("[scaled_float] only supports finite values, but got [NaN]")
    );
}

#[test]
fn the_scaling_factor_is_required_and_positive() {
    let missing = parse_error(r#"{"properties":{"s":{"type":"scaled_float"}}}"#);
    assert!(format!("{missing:?}").contains("Field [scaling_factor] is required"));
    let negative =
        parse_error(r#"{"properties":{"s":{"type":"scaled_float","scaling_factor":-1}}}"#);
    assert!(
        format!("{negative:?}").contains("[scaling_factor] must be a positive number, got [-1.0]")
    );
}

#[test]
fn a_scaled_float_mapping_writes_its_parameters_in_the_javas_order() {
    let m = mapping(
        r#"{"properties":{"s":{"type":"scaled_float","scaling_factor":100,"store":true}}}"#,
    );
    let out = m.to_json();
    assert!(
        out.contains(r#""s":{"type":"scaled_float","store":true,"scaling_factor":100.0}"#),
        "{out}"
    );
}

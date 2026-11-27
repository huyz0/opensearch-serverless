//! A `date_histogram`'s `format`: parsed, and written into `key_as_string`.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::date_histogram::DateHistogramSpec;
use super::date_histogram_render::write_date_histogram;
use super::date_histogram_tests::{part, spec_of};
use crate::aggs::partial::Format;

const JAN_1_2020: i64 = 1_577_836_800_000;
const FEB_1_2020: i64 = 1_580_515_200_000;

fn with(format: &str, offset: &str) -> DateHistogramSpec {
    spec_of(&format!(
        r#"{{"field":"t","calendar_interval":"month","format":"{format}"{offset}}}"#
    ))
    .1
}

fn written(spec: &DateHistogramSpec, format: Format) -> std::io::Result<String> {
    let result = part(spec, &[(JAN_1_2020, 2), (FEB_1_2020, 1)]);
    let mut out = Vec::new();
    write_date_histogram(&mut out, format, &result)?;
    Ok(String::from_utf8(out).expect("utf-8"))
}

#[test]
fn a_format_is_read_and_a_bucket_key_is_written_in_it() {
    let text = written(&with("yyyy-MM", ""), Format::Date).expect("writes");
    assert_eq!(
        text,
        r#""buckets":[{"key_as_string":"2020-01","key":1577836800000,"doc_count":2},{"key_as_string":"2020-02","key":1580515200000,"doc_count":1}]"#
    );
}

#[test]
fn a_keyed_answer_is_keyed_by_the_formatted_key() {
    let spec =
        spec_of(r#"{"field":"t","calendar_interval":"month","format":"dd/MM/yyyy","keyed":true}"#)
            .1;
    let text = written(&spec, Format::Date).expect("writes");
    assert!(
        text.starts_with(r#""buckets":{"01/01/2020":{"key_as_string":"01/01/2020","#),
        "{text}"
    );
}

#[test]
fn the_format_prints_the_local_time_of_the_zone() {
    let spec = spec_of(
        r#"{"field":"t","calendar_interval":"month","format":"yyyy-MM-dd HH:mm","time_zone":"+01:00"}"#,
    )
    .1;
    // The zone's midnight of 2020-01-01 is 23:00 UTC the day before.
    let result = part(&spec, &[(JAN_1_2020 - 3_600_000, 2)]);
    let mut out = Vec::new();
    write_date_histogram(&mut out, Format::Date, &result).expect("writes");
    let text = String::from_utf8(out).expect("utf-8");
    assert!(
        text.contains(r#""key_as_string":"2020-01-01 00:00""#),
        "{text}"
    );
}

#[test]
fn a_format_on_a_field_that_is_not_a_date_is_not_written() {
    assert!(written(&with("yyyy", ""), Format::Raw).is_err());
}

#[test]
fn a_format_this_node_does_not_print_is_refused() {
    let body = r#"{"aggs":{"h":{"date_histogram":{"field":"t","calendar_interval":"day","format":"yyyy-MM-dd'T'HH:mmZ"}}}}"#;
    let failure = crate::dsl::parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    assert!(
        failure.reason.contains("not supported by this node yet"),
        "{}",
        failure.reason
    );
}

//! `date_histogram` parsed, reduced and written out.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::date_histogram::{
    DateHistogramResult, DateHistogramSpec, DateRounding, DateUnit, RoundingKind, reduce,
    reduce_results,
};
use super::date_histogram_parse::time_value_millis;
use super::date_histogram_render::write_date_histogram;
use super::histogram::HistogramOrder;
use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::metric::MetricAgg;
use crate::aggs::partial::{Format, Value};
use crate::dsl::parse_chunks;

const DAY: i64 = 86_400_000;
const JAN_1_2020: i64 = 1_577_836_800_000;
const FEB_1_2020: i64 = 1_580_515_200_000;
const MAR_1_2020: i64 = 1_583_020_800_000;

fn aggs(body: &str) -> Vec<MetricAgg> {
    parse_chunks([body.as_bytes()]).expect("parses").aggs
}

pub(super) fn spec_of(body: &str) -> (String, DateHistogramSpec) {
    let body = format!(r#"{{"aggs":{{"h":{{"date_histogram":{body}}}}}}}"#);
    let agg = aggs(&body).remove(0);
    (agg.field, *agg.date_histogram.expect("a date_histogram"))
}

fn reason(body: &str) -> String {
    let body = format!(r#"{{"aggs":{{"h":{{"date_histogram":{body}}}}}}}"#);
    let failure = parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure();
    let mut text = failure.reason.clone();
    let mut cause = failure.cause.as_deref();
    while let Some(c) = cause {
        text.push_str(" | ");
        text.push_str(&c.reason);
        cause = c.cause.as_deref();
    }
    text
}

fn rounding(kind: RoundingKind) -> DateRounding {
    DateRounding {
        kind,
        ..DateHistogramSpec::default().rounding
    }
}

fn spec(kind: RoundingKind) -> DateHistogramSpec {
    DateHistogramSpec {
        rounding: rounding(kind),
        ..DateHistogramSpec::default()
    }
}

fn day() -> DateHistogramSpec {
    spec(RoundingKind::Unit(DateUnit::Day))
}

fn month() -> DateHistogramSpec {
    spec(RoundingKind::Unit(DateUnit::Month))
}

pub(super) fn part(spec: &DateHistogramSpec, buckets: &[(i64, u64)]) -> DateHistogramResult {
    DateHistogramResult {
        spec: *spec,
        buckets: buckets.to_vec(),
        ..DateHistogramResult::default()
    }
}

pub(super) fn reduced(parts: &[DateHistogramResult]) -> DateHistogramResult {
    let refs: Vec<&DateHistogramResult> = parts.iter().collect();
    reduce_results(&refs, BUCKET_LIMIT).expect("within the limit")
}

pub(super) fn written(format: Format, result: &DateHistogramResult) -> String {
    let mut out = Vec::new();
    write_date_histogram(&mut out, format, result).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

// AC-1
#[test]
fn calendar_intervals_parse_by_name_and_short_name() {
    for (text, unit) in [
        ("year", DateUnit::Year),
        ("1y", DateUnit::Year),
        ("quarter", DateUnit::Quarter),
        ("1q", DateUnit::Quarter),
        ("month", DateUnit::Month),
        ("1M", DateUnit::Month),
        ("week", DateUnit::Week),
        ("1w", DateUnit::Week),
        ("day", DateUnit::Day),
        ("1d", DateUnit::Day),
        ("hour", DateUnit::Hour),
        ("1h", DateUnit::Hour),
        ("minute", DateUnit::Minute),
        ("1m", DateUnit::Minute),
        ("second", DateUnit::Second),
        ("1s", DateUnit::Second),
    ] {
        let (field, s) = spec_of(&format!(r#"{{"field":"t","calendar_interval":"{text}"}}"#));
        assert_eq!(field, "t");
        assert_eq!(s, spec(RoundingKind::Unit(unit)), "{text}");
    }
}

#[test]
fn fixed_intervals_parse_as_time_values() {
    for (text, ms) in [
        ("10ms", 10),
        ("5s", 5_000),
        ("90m", 5_400_000),
        ("2h", 7_200_000),
        ("30d", 30 * DAY),
        ("3000micros", 3),
        ("7000000nanos", 7),
    ] {
        let (_, s) = spec_of(&format!(r#"{{"field":"t","fixed_interval":"{text}"}}"#));
        assert_eq!(s.rounding.kind, RoundingKind::Interval(ms), "{text}");
    }
}

#[test]
fn defaults_are_opensearchs() {
    let (_, s) = spec_of(r#"{"field":"t","calendar_interval":"day"}"#);
    assert!(!s.keyed);
    assert_eq!(s.min_doc_count, 0);
    assert_eq!(s.order, HistogramOrder::KeyAsc);
    assert_eq!((s.rounding.zone_ms, s.rounding.offset), (0, 0));
    assert_eq!((s.extended_bounds, s.hard_bounds), (None, (None, None)));
}

#[test]
fn zones_offsets_and_options_parse() {
    let (_, s) = spec_of(
        r#"{"field":"t","fixed_interval":"1h","time_zone":"+05:30","offset":"-6h","keyed":true,
            "min_doc_count":2,"order":{"_count":"desc"}}"#,
    );
    assert_eq!(s.rounding.zone_ms, 19_800_000);
    assert_eq!(s.rounding.offset, -21_600_000);
    assert!(s.keyed);
    assert_eq!(s.min_doc_count, 2);
    assert_eq!(s.order, HistogramOrder::CountDesc);
    for (zone, ms) in [
        ("UTC", 0),
        ("Z", 0),
        ("GMT", 0),
        ("-08:00", -28_800_000),
        ("+0530", 19_800_000),
        ("+5", 18_000_000),
        ("UTC+01:00", 3_600_000),
        ("+05:30:15", 19_815_000),
    ] {
        let (_, s) = spec_of(&format!(
            r#"{{"field":"t","calendar_interval":"day","time_zone":"{zone}"}}"#
        ));
        assert_eq!(s.rounding.zone_ms, ms, "{zone}");
    }
    let (_, s) = spec_of(r#"{"field":"t","calendar_interval":"day","time_zone":-3,"offset":500}"#);
    assert_eq!((s.rounding.zone_ms, s.rounding.offset), (-10_800_000, 500));
    let (_, s) = spec_of(r#"{"field":"t","calendar_interval":"day","offset":"+1d"}"#);
    assert_eq!(s.rounding.offset, DAY);
}

#[test]
fn bounds_round_down_without_the_offset() {
    let (_, s) = spec_of(
        r#"{"field":"t","calendar_interval":"day","offset":"2h",
            "extended_bounds":{"min":"2020-01-01T05:00:00Z","max":1578009600000},
            "hard_bounds":{"min":"2019-12-31","max":null}}"#,
    );
    assert_eq!(
        s.extended_bounds,
        Some((Some(JAN_1_2020), Some(JAN_1_2020 + 2 * DAY)))
    );
    assert_eq!(s.hard_bounds, (Some(JAN_1_2020 - DAY), None));
}

#[test]
fn bad_requests_fail_with_opensearchs_messages() {
    let cases = [
        (
            r#"{"field":"t","calendar_interval":"2d"}"#,
            "The supplied interval [2d] could not be parsed as a calendar interval.",
        ),
        (
            r#"{"field":"t","fixed_interval":"3"}"#,
            "failed to parse setting [date_histogram.fixedInterval] with value [3] as a time value: unit is missing or unrecognized",
        ),
        (
            r#"{"field":"t","fixed_interval":"1.5h"}"#,
            "failed to parse [1.5h], fractional time values are not supported",
        ),
        (
            r#"{"field":"t","fixed_interval":"abch"}"#,
            "failed to parse [abch]",
        ),
        (
            r#"{"field":"t","fixed_interval":"-5m"}"#,
            "negative durations are not supported",
        ),
        (
            r#"{"field":"t","fixed_interval":"0ms"}"#,
            "Zero or negative time interval not supported",
        ),
        (
            r#"{"field":"t","fixed_interval":"1ms","calendar_interval":"day"}"#,
            "Cannot use [calendar_interval] with [fixed_interval] configuration option.",
        ),
        (
            r#"{"field":"t"}"#,
            "Invalid interval specified, must be non-null and non-empty",
        ),
        (
            r#"{"field":"t","calendar_interval":"day","min_doc_count":-1}"#,
            "[minDocCount] must be greater than or equal to 0. Found [-1] in [h]",
        ),
        (
            r#"{"field":"t","calendar_interval":"day","extended_bounds":{"min":5,"max":1}}"#,
            "[extended_bounds.min][5] cannot be greater than [extended_bounds.max][1] for histogram aggregation [h]",
        ),
        (
            r#"{"field":"t","calendar_interval":"day","hard_bounds":{"min":"2020-01-02","max":"2020-01-01"}}"#,
            "[hard_bounds.min]",
        ),
        (
            r#"{"field":"t","calendar_interval":"day","extended_bounds":{"min":0,"max":10},
                "hard_bounds":{"min":2000000000000,"max":3000000000000}}"#,
            "Extended bounds have to be inside hard bounds, hard bounds: [2000000000000--3000000000000], extended bounds: [0--10]",
        ),
        (
            r#"{"field":"t","calendar_interval":"day","extended_bounds":{"min":"nope"}}"#,
            "failed to parse date field [nope]",
        ),
        (
            r#"{"field":"t","calendar_interval":"day","bogus":1}"#,
            "unknown field [bogus]",
        ),
        (
            r#"{"calendar_interval":"day"}"#,
            "Required one of fields [field, script]",
        ),
        (
            r#"{"field":"t","calendar_interval":["day"]}"#,
            "doesn't support values of type",
        ),
    ];
    for (body, want) in cases {
        let got = reason(body);
        assert!(got.contains(want), "{body}: {got}");
    }
}

#[test]
fn unserved_keys_zones_and_orders_are_refused_as_not_supported_yet() {
    for body in [
        r#"{"field":"t","interval":"day"}"#,
        r#"{"field":"t","calendar_interval":"day","format":"yyyy"}"#,
        r#"{"field":"t","calendar_interval":"day","missing":"2020-01-01"}"#,
        r#"{"field":"t","calendar_interval":"day","time_zone":"Europe/Nowhere"}"#,
        r#"{"field":"t","calendar_interval":"day","order":{"sub":"asc"}}"#,
        r#"{"field":"t","calendar_interval":"day","order":[{"_key":"asc"},{"_count":"asc"}]}"#,
    ] {
        assert!(
            reason(body).contains("not supported by this node yet"),
            "{body}"
        );
    }
    let body = br#"{"aggs":{"h":{"date_histogram":{"field":"t","calendar_interval":"day"},"aggs":{"m":{"percentiles":{"field":"n"}}}}}}"#;
    let e = parse_chunks([&body[..]])
        .expect_err("refused")
        .into_failure();
    assert!(
        e.reason
            .contains("[percentiles] sub-aggregation of a [date_histogram]")
    );
}

#[test]
fn time_values_follow_opensearchs_parse() {
    assert_eq!(time_value_millis("1d", "x"), Ok(DAY));
    assert_eq!(time_value_millis(" 2H ", "x"), Ok(7_200_000));
    assert_eq!(time_value_millis("5m", "x"), Ok(300_000));
    assert_eq!(time_value_millis("1500ms", "x"), Ok(1500));
    assert_eq!(time_value_millis("0", "x"), Ok(0));
    assert_eq!(time_value_millis("-1", "x"), Ok(-1));
    // `M` is months, no time value.
    assert!(time_value_millis("1M", "x").is_err());
    assert!(time_value_millis("", "x").is_err());
}

// AC-5
#[test]
fn equal_keys_add_and_empty_days_fill_between() {
    let s = day();
    let a = part(&s, &[(JAN_1_2020, 2), (JAN_1_2020 + DAY, 1)]);
    let b = part(&s, &[(JAN_1_2020 + DAY, 4), (JAN_1_2020 + 4 * DAY, 1)]);
    assert_eq!(
        reduced(&[a, b]).buckets,
        [
            (JAN_1_2020, 2),
            (JAN_1_2020 + DAY, 5),
            (JAN_1_2020 + 2 * DAY, 0),
            (JAN_1_2020 + 3 * DAY, 0),
            (JAN_1_2020 + 4 * DAY, 1),
        ]
    );
}

#[test]
fn months_fill_by_their_own_lengths() {
    let a = part(&month(), &[(JAN_1_2020, 1), (MAR_1_2020 + 31 * DAY, 1)]);
    // January, February (29 days), March (31), April.
    assert_eq!(
        reduced(&[a]).buckets,
        [
            (JAN_1_2020, 1),
            (FEB_1_2020, 0),
            (MAR_1_2020, 0),
            (MAR_1_2020 + 31 * DAY, 1)
        ]
    );
}

#[test]
fn min_doc_count_above_zero_drops_buckets_and_makes_no_gaps() {
    let mut s = day();
    s.min_doc_count = 2;
    let a = part(
        &s,
        &[
            (JAN_1_2020, 2),
            (JAN_1_2020 + DAY, 1),
            (JAN_1_2020 + 5 * DAY, 3),
        ],
    );
    assert_eq!(
        reduced(&[a]).buckets,
        [(JAN_1_2020, 2), (JAN_1_2020 + 5 * DAY, 3)]
    );
}

#[test]
fn extended_bounds_fill_before_and_after_the_data() {
    let mut s = day();
    s.extended_bounds = Some((Some(JAN_1_2020), Some(JAN_1_2020 + 4 * DAY)));
    let a = part(&s, &[(JAN_1_2020 + 2 * DAY, 1)]);
    assert_eq!(
        reduced(&[a]).buckets,
        [
            (JAN_1_2020, 0),
            (JAN_1_2020 + DAY, 0),
            (JAN_1_2020 + 2 * DAY, 1),
            (JAN_1_2020 + 3 * DAY, 0),
            (JAN_1_2020 + 4 * DAY, 0),
        ]
    );
    // With no data the bounds alone make the buckets, both ends needed.
    assert_eq!(reduced(&[part(&s, &[])]).buckets.len(), 5);
    s.extended_bounds = Some((Some(JAN_1_2020), None));
    assert!(reduced(&[part(&s, &[])]).buckets.is_empty());
    // A bound inside the data makes no bucket.
    s.extended_bounds = Some((Some(JAN_1_2020 + 2 * DAY), Some(JAN_1_2020)));
    let a = part(&s, &[(JAN_1_2020 + DAY, 1)]);
    assert_eq!(reduced(&[a]).buckets, [(JAN_1_2020 + DAY, 1)]);
}

#[test]
fn an_offset_moves_the_bounds_and_the_fill() {
    let mut s = day();
    s.rounding.offset = 2 * 3_600_000;
    s.extended_bounds = Some((Some(JAN_1_2020), Some(JAN_1_2020 + DAY)));
    let key = JAN_1_2020 + 2 * 3_600_000;
    let a = part(&s, &[(key + DAY, 1)]);
    assert_eq!(reduced(&[a]).buckets, [(key, 0), (key + DAY, 1)]);
    let a = part(&s, &[(key - DAY, 1), (key + 2 * DAY, 1)]);
    assert_eq!(
        reduced(&[a]).buckets,
        [(key - DAY, 1), (key, 0), (key + DAY, 0), (key + 2 * DAY, 1)]
    );
}

#[test]
fn orders_list_as_opensearch_lists_them() {
    let mut s = day();
    let data = |s: &DateHistogramSpec| part(s, &[(0, 5), (DAY, 2), (2 * DAY, 9), (3 * DAY, 7)]);
    s.order = HistogramOrder::KeyDesc;
    assert_eq!(
        reduced(&[data(&s)]).buckets,
        [(3 * DAY, 7), (2 * DAY, 9), (DAY, 2), (0, 5)]
    );
    s.order = HistogramOrder::CountDesc;
    assert_eq!(
        reduced(&[data(&s)]).buckets,
        [(2 * DAY, 9), (3 * DAY, 7), (0, 5), (DAY, 2)]
    );
    s.order = HistogramOrder::CountAsc;
    assert_eq!(
        reduced(&[data(&s)]).buckets,
        [(DAY, 2), (0, 5), (3 * DAY, 7), (2 * DAY, 9)]
    );
}

#[test]
fn a_fill_past_the_bucket_limit_is_refused_not_made() {
    let s = spec(RoundingKind::Unit(DateUnit::Second));
    let a = part(&s, &[(0, 1), (DAY * 365, 1)]);
    assert_eq!(
        reduce_results(&[&a], BUCKET_LIMIT).expect_err("too many"),
        BUCKET_LIMIT + 1
    );
    let a = part(&s, &[(0, 1), (65_534_000, 1)]);
    assert_eq!(
        reduce_results(&[&a], BUCKET_LIMIT)
            .expect("fits")
            .buckets
            .len(),
        BUCKET_LIMIT
    );
    let over = Value::DateHistogram(Box::new(part(&s, &[(0, 1), (DAY * 365, 1)])));
    assert!(reduce(&[over]).is_none());
}

#[test]
fn reduce_picks_only_date_histograms() {
    let h = Value::DateHistogram(Box::new(part(&day(), &[(0, 1)])));
    assert!(reduce(&[]).is_none());
    assert!(reduce(&[h.clone(), Value::Min(1.0)]).is_none());
    let Some(Value::DateHistogram(got)) = crate::aggs::partial::reduce(&[h.clone(), h]) else {
        panic!("a date histogram");
    };
    assert_eq!(got.buckets, [(0, 2)]);
}

// AC-6
#[test]
fn a_date_fields_keys_carry_text_before_the_key() {
    let r = part(&day(), &[(JAN_1_2020, 1), (JAN_1_2020 + 2 * DAY, 3)]);
    assert_eq!(
        written(Format::Date, &r),
        r#""buckets":[{"key_as_string":"2020-01-01T00:00:00.000Z","key":1577836800000,"doc_count":1},{"key_as_string":"2020-01-02T00:00:00.000Z","key":1577923200000,"doc_count":0},{"key_as_string":"2020-01-03T00:00:00.000Z","key":1578009600000,"doc_count":3}]"#
    );
}

#[test]
fn keys_print_in_the_aggregations_zone() {
    let mut s = day();
    s.rounding.zone_ms = 19_800_000;
    s.keyed = true;
    // Local midnight at +05:30 is the previous day 18:30 UTC.
    let key = JAN_1_2020 - 19_800_000;
    let r = part(&s, &[(key, 1)]);
    assert_eq!(
        written(Format::Date, &r),
        r#""buckets":{"2020-01-01T00:00:00.000+05:30":{"key_as_string":"2020-01-01T00:00:00.000+05:30","key":1577817000000,"doc_count":1}}"#
    );
    s.rounding.zone_ms = -28_800_000;
    let r = part(&s, &[(JAN_1_2020 + 8 * 3_600_000, 1)]);
    assert!(
        written(Format::Date, &r).contains(r#""key_as_string":"2020-01-01T00:00:00.000-08:00""#)
    );
}

#[test]
fn a_long_fields_keys_are_numbers_alone() {
    let mut s = day();
    let r = part(&s, &[(0, 1)]);
    assert_eq!(
        written(Format::Raw, &r),
        r#""buckets":[{"key":0,"doc_count":1}]"#
    );
    s.keyed = true;
    let r = part(&s, &[(DAY, 2)]);
    assert_eq!(
        written(Format::Raw, &r),
        r#""buckets":{"86400000":{"key":86400000,"doc_count":2}}"#
    );
    assert_eq!(written(Format::Raw, &part(&day(), &[])), r#""buckets":[]"#);
}

#[test]
fn a_result_over_the_bucket_limit_fails_to_write() {
    let s = spec(RoundingKind::Unit(DateUnit::Second));
    let r = part(&s, &[(0, 1), (DAY * 365, 1)]);
    let mut out = Vec::new();
    assert!(write_date_histogram(&mut out, Format::Date, &r).is_err());
}

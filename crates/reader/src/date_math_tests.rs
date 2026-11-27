use super::*;
use crate::date::{DateFormat, parse_to_millis, parse_zone};

const NOW: i64 = 1_025_518_530_123; // 2002-07-01T10:15:30.123Z

fn at(text: &str) -> Result<i64, DateError> {
    parse_at(text, false, Some(NOW))
}

fn up(text: &str) -> Result<i64, DateError> {
    parse_at(text, true, Some(NOW))
}

fn d(text: &str) -> i64 {
    parse_to_millis(text, false).expect("date")
}

#[test]
fn now_is_the_given_instant_and_not_a_clock() {
    assert_eq!(at("now"), Ok(NOW));
    assert_eq!(parse_at("now", false, None), Err(DateError::Math));
}

#[test]
fn units_add_and_subtract() {
    assert_eq!(at("now-1h"), Ok(NOW - 3_600_000));
    assert_eq!(at("now+90m"), Ok(NOW + 90 * 60_000));
    assert_eq!(at("now-1d+1s"), Ok(NOW - DAY + 1000));
    assert_eq!(at("now+2w"), Ok(NOW + 14 * DAY));
    assert_eq!(at("now+1"), Err(DateError::Unparsable));
    assert_eq!(at("now+1x"), Err(DateError::Unparsable));
    assert_eq!(at("now*1d"), Err(DateError::Unparsable));
}

#[test]
fn a_bare_unit_is_one() {
    assert_eq!(at("now-d"), Ok(NOW - DAY));
}

#[test]
fn months_and_years_clamp_to_the_month_end() {
    assert_eq!(at("2001-01-31||+1M"), Ok(d("2001-02-28")));
    assert_eq!(at("2000-01-31||+1M"), Ok(d("2000-02-29")));
    assert_eq!(
        at("2001-03-31T05:00:00Z||-1M"),
        Ok(d("2001-02-28T05:00:00Z"))
    );
    assert_eq!(at("2000-02-29||+1y"), Ok(d("2001-02-28")));
    assert_eq!(at("1970-01-01||-1M"), Ok(d("1969-12-01")));
}

#[test]
fn rounding_takes_the_start_or_the_last_millisecond() {
    assert_eq!(at("now/d"), Ok(d("2002-07-01")));
    assert_eq!(up("now/d"), Ok(d("2002-07-02") - 1));
    assert_eq!(at("now/h"), Ok(d("2002-07-01T10:00:00Z")));
    assert_eq!(up("now/m"), Ok(d("2002-07-01T10:15:59.999Z")));
    assert_eq!(at("now/s"), Ok(d("2002-07-01T10:15:30Z")));
    assert_eq!(at("now/M"), Ok(d("2002-07-01")));
    assert_eq!(up("now/M"), Ok(d("2002-08-01") - 1));
    assert_eq!(at("now/y"), Ok(d("2002-01-01")));
    assert_eq!(up("now/y"), Ok(d("2003-01-01") - 1));
    assert_eq!(at("now-1d/d"), Ok(d("2002-06-30")));
}

#[test]
fn a_week_starts_on_monday() {
    // 2002-07-01 is a Monday; 2002-07-03 a Wednesday.
    assert_eq!(at("2002-07-03||/w"), Ok(d("2002-07-01")));
    assert_eq!(up("2002-07-03||/w"), Ok(d("2002-07-08") - 1));
    assert_eq!(at("2002-07-07||/w"), Ok(d("2002-07-01")));
}

#[test]
fn an_anchor_needs_no_instant() {
    assert_eq!(
        parse_at("2001-03-04||+1d", false, None),
        Ok(d("2001-03-05"))
    );
    assert_eq!(parse_at("2001-03-04||", false, None), Ok(d("2001-03-04")));
    assert_eq!(
        parse_at("2001-03-04||x", false, None),
        Err(DateError::Unparsable)
    );
}

#[test]
fn text_without_math_reads_as_before() {
    assert_eq!(at("2001-03-04"), Ok(d("2001-03-04")));
    assert_eq!(up("2001-03-04"), Ok(d("2001-03-05") - 1));
}

#[test]
fn an_instant_at_the_ends_of_the_range_is_refused_or_read_and_never_panics() {
    for now in [i64::MIN, i64::MAX, 0] {
        for text in [
            "now", "now/y", "now/M", "now/w", "now/d", "now+1y", "now-1M", "now+1d", "now/h",
        ] {
            let _ = parse_at(text, false, Some(now));
            let _ = parse_at(text, true, Some(now));
        }
    }
}

#[test]
fn civil_inverts_days_from_civil_over_every_month_of_many_years() {
    for year in (-500..=2600)
        .step_by(7)
        .chain([1600, 1900, 2000, 2024, 2100])
    {
        for month in 1..=12 {
            let last = days_in_month(year, month);
            for day in [1, 2, 15, 28, last] {
                let days = days_from_civil(year, month, day);
                assert_eq!(civil(days), (year, month, day), "{year}-{month}-{day}");
            }
        }
    }
}

#[test]
fn a_range_bound_includes_or_steps_past_the_instant() {
    // `gte` takes the instant, `gt` the millisecond after the rounded-up one.
    assert_eq!(lower_bound_at("now", true, Some(NOW)), Ok(NOW));
    assert_eq!(lower_bound_at("now", false, Some(NOW)), Ok(NOW + 1));
    assert_eq!(
        lower_bound_at("now/d", true, Some(NOW)),
        Ok(d("2002-07-01"))
    );
    assert_eq!(
        lower_bound_at("now/d", false, Some(NOW)),
        Ok(d("2002-07-02"))
    );
    // `lte` takes the rounded-up instant, `lt` the millisecond before it.
    assert_eq!(upper_bound_at("now", true, Some(NOW)), Ok(NOW));
    assert_eq!(upper_bound_at("now", false, Some(NOW)), Ok(NOW - 1));
    assert_eq!(
        upper_bound_at("now/d", true, Some(NOW)),
        Ok(d("2002-07-02") - 1)
    );
    assert_eq!(
        upper_bound_at("now/d", false, Some(NOW)),
        Ok(d("2002-07-01") - 1)
    );
}

fn in_zone(text: &str, round_up: bool, zone: i64) -> Result<i64, DateError> {
    let spec = DateSpec {
        zone,
        named: true,
        ..DateSpec::default()
    };
    parse_at_with(text, round_up, Some(NOW), spec)
}

#[test]
fn a_zone_reads_a_bound_with_none_of_its_own_and_leaves_one_that_has_one() {
    let hour = 3_600;
    let utc = d("2002-07-01T10:00:00Z");
    assert_eq!(
        in_zone("2002-07-01T10:00:00", false, hour),
        Ok(utc - 3_600_000)
    );
    assert_eq!(
        in_zone("2002-07-01T10:00:00", false, -5 * hour),
        Ok(utc + 5 * 3_600_000)
    );
    assert_eq!(in_zone("2002-07-01T10:00:00Z", false, 5 * hour), Ok(utc));
    assert_eq!(
        in_zone("2002-07-01T10:00:00+02:00", false, 5 * hour),
        Ok(utc - 2 * 3_600_000)
    );
    // An epoch-millisecond bound is an instant already.
    assert_eq!(in_zone("1025518530123", false, 5 * hour), Ok(NOW));
}

#[test]
fn rounding_and_arithmetic_run_on_the_calendar_of_the_zone() {
    let hour = 3_600;
    // 2002-07-01T10:15:30Z is 12:15:30 in +02:00: its day starts at 2002-06-30T22:00Z.
    assert_eq!(
        in_zone("now/d", false, 2 * hour),
        Ok(d("2002-06-30T22:00:00Z"))
    );
    assert_eq!(
        in_zone("now/d", true, 2 * hour),
        Ok(d("2002-07-01T22:00:00Z") - 1)
    );
    // 10:15:30Z is 05:15:30 in -05:00: still the day before in UTC terms of the zone's day start.
    assert_eq!(
        in_zone("now/d", false, -5 * hour),
        Ok(d("2002-07-01T05:00:00Z"))
    );
    assert_eq!(
        in_zone("now-1d/d", false, -5 * hour),
        Ok(d("2002-06-30T05:00:00Z"))
    );
    // The zone's day, not the UTC one, is what an anchor is rounded to.
    assert_eq!(
        in_zone("2002-07-01T01:00:00||/d", false, 2 * hour),
        Ok(d("2002-06-30T22:00:00Z"))
    );
    // No zone is the UTC arithmetic.
    assert_eq!(in_zone("now/d", false, 0), at("now/d"));
}

#[test]
fn a_forced_format_reads_only_its_own_text() {
    let spec = |format| DateSpec {
        format,
        named: true,
        ..DateSpec::default()
    };
    let read = |text: &str, format| parse_at_with(text, false, None, spec(format));
    assert_eq!(read("2002-07-01", DateFormat::Iso), Ok(d("2002-07-01")));
    assert_eq!(
        read("1025518530123", DateFormat::Iso),
        Err(DateError::Unparsable)
    );
    assert_eq!(read("1025518530123", DateFormat::Epoch), Ok(NOW));
    assert_eq!(
        read("2002-07-01", DateFormat::Epoch),
        Err(DateError::Unparsable)
    );
    assert_eq!(read("2002-07-01", DateFormat::Default), Ok(d("2002-07-01")));
    assert_eq!(read("1025518530123", DateFormat::Default), Ok(NOW));
}

#[test]
fn a_zone_is_a_fixed_one_this_node_reads() {
    for (text, seconds) in [
        ("Z", Some(0)),
        ("UTC", Some(0)),
        ("GMT", Some(0)),
        ("+01:00", Some(3_600)),
        ("-05:30", Some(-19_800)),
        ("+0200", Some(7_200)),
        ("+09", Some(32_400)),
        ("Asia/Kolkata", None),
        ("+25:00", None),
        ("+01:00:00", None),
        ("", None),
        ("1", None),
    ] {
        assert_eq!(parse_zone(text), seconds, "{text}");
    }
}

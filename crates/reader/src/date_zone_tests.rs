use super::*;
use crate::date::{DateSpec, parse_to_millis_with};
use crate::date_math::parse_at_with;

fn paris() -> DateSpec {
    DateSpec {
        region: parse_time_zone("Europe/Paris").and_then(|z| z.region),
        named: true,
        ..DateSpec::default()
    }
}

fn utc(text: &str) -> i64 {
    parse_to_millis_with(text, false, DateSpec::default()).expect("a date")
}

fn read(text: &str) -> i64 {
    parse_to_millis_with(text, false, paris()).expect("a bound")
}

fn at(text: &str, now: &str) -> i64 {
    parse_at_with(text, false, Some(utc(now)), paris()).expect("a bound")
}

#[test]
fn a_region_reads_wall_clock_time_at_the_offset_in_force() {
    assert_eq!(read("2002-01-15T10:00:00"), utc("2002-01-15T09:00:00Z"));
    assert_eq!(read("2002-07-15T10:00:00"), utc("2002-07-15T08:00:00Z"));
    // A bound with its own offset, and an epoch-millisecond one, ignore the zone.
    assert_eq!(read("2002-07-15T10:00:00Z"), utc("2002-07-15T10:00:00Z"));
    assert_eq!(read("1025518530123"), 1_025_518_530_123);
}

#[test]
fn a_region_day_is_as_long_as_the_calendar_makes_it() {
    // 2002-03-31 is the 23-hour spring-forward day in Paris, 2002-10-27 the 25-hour fall-back one.
    let midnight = |text: &str, now: &str| at(text, now);
    let a = midnight("now/d", "2002-03-30T12:00:00Z");
    let b = midnight("now/d", "2002-03-31T12:00:00Z");
    let c = midnight("now/d", "2002-04-01T12:00:00Z");
    assert_eq!(a, utc("2002-03-29T23:00:00Z"));
    assert_eq!(b - a, 24 * 3_600_000);
    assert_eq!(c - b, 23 * 3_600_000);
    let d = midnight("now/d", "2002-10-27T12:00:00Z");
    let e = midnight("now/d", "2002-10-28T12:00:00Z");
    assert_eq!(e - d, 25 * 3_600_000);
    // `+1d` keeps the wall-clock time.
    assert_eq!(
        at("2002-03-30T12:00:00||+1d", "2002-01-01T00:00:00Z"),
        utc("2002-03-31T10:00:00Z")
    );
}

#[test]
fn a_gap_shifts_forward_and_an_overlap_takes_the_earlier_offset() {
    // 02:30 on 2002-03-31 does not exist in Paris: it reads as 03:30 (+02:00).
    assert_eq!(read("2002-03-31T02:30:00"), utc("2002-03-31T01:30:00Z"));
    // 02:30 on 2002-10-27 happens twice: the earlier is +02:00.
    assert_eq!(read("2002-10-27T02:30:00"), utc("2002-10-27T00:30:00Z"));
}

#[test]
fn only_a_region_the_database_spells_is_read() {
    assert!(parse_time_zone("Europe/Paris").is_some());
    assert!(parse_time_zone("+01:00").is_some_and(|z| z.region.is_none()));
    for text in ["europe/paris", "Mars/Base", "", "1"] {
        assert!(parse_time_zone(text).is_none(), "{text}");
    }
}

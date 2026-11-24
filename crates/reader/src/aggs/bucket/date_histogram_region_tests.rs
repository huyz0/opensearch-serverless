//! `date_histogram` in an IANA region time zone: parsed, filled and written.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use super::date_histogram_tests::{part, reduced, spec_of, written};
use crate::aggs::partial::Format;

const JAN_1_2020: i64 = 1_577_836_800_000;

// AC-1
#[test]
fn a_region_zone_parses_to_its_rules() {
    let (_, s) = spec_of(
        r#"{"field":"t","calendar_interval":"day","time_zone":"Europe/Paris","offset":"+1h"}"#,
    );
    let zone = s.rounding.region.expect("a region zone");
    assert_eq!(zone.id(), "Europe/Paris");
    assert_eq!((s.rounding.zone_ms, s.rounding.offset), (0, 3_600_000));
    // A later fixed offset replaces it, and the other way round.
    let (_, s) = spec_of(
        r#"{"field":"t","calendar_interval":"day","time_zone":"Europe/Paris","time_zone":"+02:00"}"#,
    );
    assert_eq!((s.rounding.region, s.rounding.zone_ms), (None, 7_200_000));
    let (_, s) = spec_of(
        r#"{"field":"t","calendar_interval":"day","time_zone":"+02:00","time_zone":"Asia/Tokyo"}"#,
    );
    assert_eq!(s.rounding.zone_ms, 0);
    assert!(s.rounding.region.is_some());
    // Bounds round down by the zone's days.
    let (_, s) = spec_of(
        r#"{"field":"t","calendar_interval":"day","time_zone":"Europe/Paris","hard_bounds":{"min":1616886000001,"max":1616970000000}}"#,
    );
    assert_eq!(
        s.hard_bounds,
        (Some(1_616_886_000_000), Some(1_616_968_800_000))
    );
}

#[test]
fn a_region_id_in_another_case_is_refused() {
    let body = br#"{"aggs":{"h":{"date_histogram":{"field":"t","calendar_interval":"day","time_zone":"europe/paris"}}}}"#;
    let e = crate::dsl::parse_chunks([&body[..]])
        .expect_err("refused")
        .into_failure();
    assert!(
        e.reason.contains("not supported by this node yet"),
        "{}",
        e.reason
    );
}

// AC-3
#[test]
fn a_region_zones_days_fill_by_their_own_lengths() {
    // Paris days 2021-03-27 (24 h), 03-28 (23 h, the clocks go forward) and 03-29.
    let (_, s) = spec_of(r#"{"field":"t","calendar_interval":"day","time_zone":"Europe/Paris"}"#);
    let (mar27, mar28, mar29) = (1_616_799_600_000, 1_616_886_000_000, 1_616_968_800_000);
    let a = part(&s, &[(mar27, 1), (mar29, 2)]);
    assert_eq!(reduced(&[a]).buckets, [(mar27, 1), (mar28, 0), (mar29, 2)]);
    // Fall back: 2021-10-31 is 25 hours.
    let (oct30, oct31, nov1) = (1_635_544_800_000, 1_635_631_200_000, 1_635_721_200_000);
    let a = part(&s, &[(oct30, 1), (nov1, 2)]);
    assert_eq!(reduced(&[a]).buckets, [(oct30, 1), (oct31, 0), (nov1, 2)]);
}

// AC-4
#[test]
fn keys_print_in_a_region_zones_offset_at_the_key() {
    let (_, mut s) =
        spec_of(r#"{"field":"t","calendar_interval":"day","time_zone":"Europe/Paris"}"#);
    s.keyed = true;
    let r = part(&s, &[(1_616_799_600_000, 1), (1_616_968_800_000, 2)]);
    let text = written(Format::Date, &r);
    assert!(
        text.contains(r#""key_as_string":"2021-03-27T00:00:00.000+01:00""#),
        "{text}"
    );
    assert!(
        text.contains(r#""key_as_string":"2021-03-29T00:00:00.000+02:00""#),
        "{text}"
    );
    // London in winter is at a zero offset: printed `Z`.
    let (_, s) = spec_of(r#"{"field":"t","calendar_interval":"day","time_zone":"Europe/London"}"#);
    let r = part(&s, &[(JAN_1_2020, 1)]);
    assert!(written(Format::Date, &r).contains(r#""key_as_string":"2020-01-01T00:00:00.000Z""#));
}

//! Date math over a `date` field's default format: `now`, `anchor||` and the
//! `+1d`, `-2M`, `/d` operations after them, evaluated at a given instant.
//!
//! Ported from `JavaDateMathParser.parse` and `parseMath`
//! (server/src/main/java/org/opensearch/common/time/) at OpenSearch 3.10.0,
//! in UTC. `now` is the instant the caller passes, which a forwarded search
//! takes from the request, never from this node's clock.

use crate::date::{DateError, DateSpec, days_from_civil, days_in_month, parse_to_millis_with};

const DAY: i64 = 86_400_000;
/// The first millisecond of year -9999 and the last of year 9999.
const MIN: i64 = -377_705_116_800_000;
const MAX: i64 = 253_402_300_799_999;

/// `text` as epoch milliseconds, with date math evaluated at `now`. Text
/// without `now` or `||` is read as [`parse_to_millis`](crate::date::parse_to_millis) reads it. Rounding a
/// day, month or year with `/` takes its start, or with `round_up` its last
/// millisecond.
///
/// # Errors
///
/// [`DateError::Math`] for `now` when no instant is given;
/// [`DateError::Unparsable`] for an anchor or an operation that is not valid.
pub fn parse_at(text: &str, round_up: bool, now: Option<i64>) -> Result<i64, DateError> {
    parse_at_with(text, round_up, now, DateSpec::default())
}

/// [`parse_at`] in the format and zone of `spec`: an anchor with no zone of its own is read in
/// the zone, and the operations (`+1d`, `/d`) run on the calendar of that zone, so `now/d` is the
/// start of the zone's day.
///
/// # Errors
///
/// As [`parse_at`].
pub fn parse_at_with(
    text: &str,
    round_up: bool,
    now: Option<i64>,
    spec: DateSpec,
) -> Result<i64, DateError> {
    let (base, math) = if let Some(rest) = text.strip_prefix("now") {
        (now.ok_or(DateError::Math)?, rest)
    } else if let Some((anchor, math)) = text.split_once("||") {
        (parse_to_millis_with(anchor, round_up, spec)?, math)
    } else {
        return parse_to_millis_with(text, round_up, spec);
    };
    // ARITH: an offset is at most a day; the saturation keeps an extreme instant in range.
    let offset = spec.offset_at(base);
    apply(base.saturating_add(offset), math, round_up).map(|local| spec.utc_of_local(local))
}

#[derive(Clone, Copy, PartialEq)]
enum Unit {
    Year,
    Month,
    Week,
    Day,
    Hour,
    Minute,
    Second,
}

impl Unit {
    fn of(c: char) -> Option<Self> {
        Some(match c {
            'y' => Self::Year,
            'M' => Self::Month,
            'w' => Self::Week,
            'd' => Self::Day,
            'h' | 'H' => Self::Hour,
            'm' => Self::Minute,
            's' => Self::Second,
            _ => return None,
        })
    }
}

/// The operations of `math` over `base`: `+n<unit>`, `-n<unit>` and `/<unit>`.
/// The lower bound of a range at `now`: `gte` as given, `gt` rounded up and
/// one millisecond past it.
///
/// # Errors
///
/// As [`parse_at`].
pub fn lower_bound_at(text: &str, include: bool, now: Option<i64>) -> Result<i64, DateError> {
    lower_bound_with(text, include, now, DateSpec::default())
}

/// [`lower_bound_at`] in the format and zone of `spec`.
///
/// # Errors
///
/// As [`parse_at`].
pub fn lower_bound_with(
    text: &str,
    include: bool,
    now: Option<i64>,
    spec: DateSpec,
) -> Result<i64, DateError> {
    let millis = parse_at_with(text, !include, now, spec)?;
    Ok(if include {
        millis
    } else {
        millis.saturating_add(1)
    })
}

/// The upper bound of a range at `now`: `lte` rounded up, `lt` as given and
/// one millisecond before it.
///
/// # Errors
///
/// As [`parse_at`].
pub fn upper_bound_at(text: &str, include: bool, now: Option<i64>) -> Result<i64, DateError> {
    upper_bound_with(text, include, now, DateSpec::default())
}

/// [`upper_bound_at`] in the format and zone of `spec`.
///
/// # Errors
///
/// As [`parse_at`].
pub fn upper_bound_with(
    text: &str,
    include: bool,
    now: Option<i64>,
    spec: DateSpec,
) -> Result<i64, DateError> {
    let millis = parse_at_with(text, include, now, spec)?;
    Ok(if include {
        millis
    } else {
        millis.saturating_sub(1)
    })
}

fn apply(base: i64, math: &str, round_up: bool) -> Result<i64, DateError> {
    let mut at = base;
    let mut rest = math;
    while let Some(op) = rest.chars().next() {
        rest = &rest[1..];
        // The arithmetic below is for years of at most four digits.
        if !(MIN..=MAX).contains(&at) {
            return Err(DateError::Unparsable);
        }
        if op == '/' {
            let unit = take_unit(&mut rest)?;
            let floor = floor(at, unit)?;
            at = if round_up {
                shift(floor, unit, 1)?
                    .checked_sub(1)
                    .ok_or(DateError::Unparsable)?
            } else {
                floor
            };
        } else if op == '+' || op == '-' {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let n: i64 = if digits == 0 {
                1
            } else {
                rest[..digits].parse().map_err(|_| DateError::Unparsable)?
            };
            rest = &rest[digits..];
            let unit = take_unit(&mut rest)?;
            at = shift(at, unit, if op == '+' { n } else { n.saturating_neg() })?;
        } else {
            return Err(DateError::Unparsable);
        }
    }
    Ok(at)
}

fn take_unit(rest: &mut &str) -> Result<Unit, DateError> {
    let c = rest.chars().next().ok_or(DateError::Unparsable)?;
    *rest = &rest[c.len_utf8()..];
    Unit::of(c).ok_or(DateError::Unparsable)
}

// ARITH: every value is checked or bounded by a year of at most four digits.
#[allow(clippy::arithmetic_side_effects)]
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `at` moved by `n` units; months and years keep the day, or the month's
/// last day when it is shorter, as `ZonedDateTime.plusMonths` does.
// ARITH: the year is at most four digits and the month index is checked; the
// day arithmetic is on values the year bound keeps within a few trillion.
#[allow(clippy::arithmetic_side_effects)]
fn shift(at: i64, unit: Unit, n: i64) -> Result<i64, DateError> {
    let fixed = |ms: i64| {
        n.checked_mul(ms)
            .and_then(|d| at.checked_add(d))
            .ok_or(DateError::Unparsable)
    };
    match unit {
        Unit::Week => fixed(7 * DAY),
        Unit::Day => fixed(DAY),
        Unit::Hour => fixed(3_600_000),
        Unit::Minute => fixed(60_000),
        Unit::Second => fixed(1_000),
        Unit::Year | Unit::Month => {
            let months = if unit == Unit::Year {
                n.checked_mul(12)
            } else {
                Some(n)
            }
            .ok_or(DateError::Unparsable)?;
            let (year, month, day) = civil(at.div_euclid(DAY));
            let index = (year * 12 + month - 1)
                .checked_add(months)
                .ok_or(DateError::Unparsable)?;
            let (y, m) = (index.div_euclid(12), index.rem_euclid(12) + 1);
            if !(-9_999..=9_999).contains(&y) {
                return Err(DateError::Unparsable);
            }
            let d = day.min(days_in_month(y, m));
            Ok(days_from_civil(y, m, d) * DAY + at.rem_euclid(DAY))
        }
    }
}

/// The start of the unit `at` falls in.
// ARITH: `at` is within a four-digit year, so a day count times a day's
// milliseconds stays far inside an i64.
#[allow(clippy::arithmetic_side_effects)]
fn floor(at: i64, unit: Unit) -> Result<i64, DateError> {
    let days = at.div_euclid(DAY);
    let (year, month, _) = civil(days);
    Ok(match unit {
        Unit::Year => days_from_civil(year, 1, 1) * DAY,
        Unit::Month => days_from_civil(year, month, 1) * DAY,
        // 1970-01-01 was a Thursday, three days after a Monday.
        Unit::Week => (days - (days + 3).rem_euclid(7)) * DAY,
        Unit::Day => days * DAY,
        Unit::Hour => at - at.rem_euclid(3_600_000),
        Unit::Minute => at - at.rem_euclid(60_000),
        Unit::Second => at - at.rem_euclid(1_000),
    })
}

#[cfg(test)]
#[path = "date_math_tests.rs"]
mod tests;

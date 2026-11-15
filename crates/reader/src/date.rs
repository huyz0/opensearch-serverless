//! Dates in a `date` field's default format, as epoch milliseconds.
//!
//! Ported from `DateFormatters.STRICT_DATE_OPTIONAL_TIME`, `EpochTime`'s
//! millisecond parser and `JavaDateMathParser.parse`'s round-up
//! (server/src/main/java/org/opensearch/common/time/), with the bounds of
//! `DateFieldMapper.DateFieldType.rangeQuery`
//! (server/src/main/java/org/opensearch/index/mapper/DateFieldMapper.java), at
//! OpenSearch 3.10.0. The format is
//! `strict_date_optional_time||epoch_millis`: the first parser that accepts
//! the text wins. A rounded-up date fills each missing field with its
//! largest value except the month and the day, which default to 1, as
//! `JavaDateFormatter`'s round-up defaults do. Date math (`now`, `||`) is not
//! served yet.

use crate::failure::Failure;

/// The default format's name, as errors quote it.
pub const DEFAULT_FORMAT: &str = "strict_date_optional_time||epoch_millis";

/// Why a date could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DateError {
    /// No parser of the format accepts the text.
    Unparsable,
    /// Date math, which this node does not serve yet.
    Math,
}

impl DateError {
    /// The exception OpenSearch answers with for `text`.
    pub fn into_failure(self, text: &str) -> Failure {
        match self {
            DateError::Unparsable => {
                // ALLOC: error path; one message.
                let inner =
                    format!("failed to parse date field [{text}] with format [{DEFAULT_FORMAT}]");
                // ALLOC: error path; one message.
                Failure::opensearch("parse_exception", format!("{inner}: [{inner}]")).caused_by(
                    Failure::plain("illegal_argument_exception", inner).caused_by(Failure::plain(
                        "date_time_parse_exception",
                        "Failed to parse with all enclosed parsers",
                    )),
                )
            }
            DateError::Math => Failure::opensearch(
                "parse_exception",
                // ALLOC: error path; one message.
                format!("date math in [{text}] is not supported by this node yet"),
            ),
        }
    }
}

/// `DateFieldType.parseToLong`: `text` in the default format, as epoch
/// milliseconds, rounding missing fields up when `round_up`.
pub fn parse_to_millis(text: &str, round_up: bool) -> Result<i64, DateError> {
    if text.starts_with("now") || text.contains("||") {
        return Err(DateError::Math);
    }
    if let Some(millis) = strict_date_optional_time(text.as_bytes(), round_up) {
        return Ok(millis);
    }
    epoch_millis(text).ok_or(DateError::Unparsable)
}

/// The lower bound of a range: `gte` as given, `gt` rounded up and one
/// millisecond past it.
pub fn lower_bound(text: &str, include: bool) -> Result<i64, DateError> {
    let millis = parse_to_millis(text, !include)?;
    Ok(if include {
        millis
    } else {
        millis.saturating_add(1)
    })
}

/// The upper bound of a range: `lte` rounded up, `lt` as given and one
/// millisecond before it.
pub fn upper_bound(text: &str, include: bool) -> Result<i64, DateError> {
    let millis = parse_to_millis(text, include)?;
    Ok(if include {
        millis
    } else {
        millis.saturating_sub(1)
    })
}

/// `EpochTime.MILLIS_FORMATTER`: an optional sign, digits, and an optional
/// fraction of a millisecond, which an epoch-millisecond value drops
/// (toward negative infinity, as `Instant.toEpochMilli` does).
fn epoch_millis(text: &str) -> Option<i64> {
    let (whole, fraction) = match text.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (text, None),
    };
    let digits = whole.strip_prefix('-').unwrap_or(whole);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let millis: i64 = whole.parse().ok()?;
    let Some(fraction) = fraction else {
        return Some(millis);
    };
    if fraction.is_empty() || fraction.len() > 6 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let below = fraction.bytes().any(|b| b != b'0');
    Some(if below && whole.starts_with('-') {
        millis.checked_sub(1)?
    } else {
        millis
    })
}

/// A cursor over the text's bytes.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.at = self.at.saturating_add(1);
            true
        } else {
            false
        }
    }

    /// Exactly `n` digits.
    fn digits(&mut self, n: usize) -> Option<i64> {
        let mut value = 0i64;
        for _ in 0..n {
            let d = self.peek().filter(u8::is_ascii_digit)?;
            value = value
                .checked_mul(10)?
                .checked_add(i64::from(d.wrapping_sub(b'0')))?;
            self.at = self.at.saturating_add(1);
        }
        Some(value)
    }

    fn done(&self) -> bool {
        self.at >= self.bytes.len()
    }
}

/// `strict_date_optional_time`: `yyyy[-MM[-dd]]`, then optionally `T` and
/// `HH[:mm[:ss[.S{1,9}]]]` (a comma also separates the fraction), then an
/// optional zone: `Z`, `+HH`, `+HHmm` or `+HH:mm`. Without a zone the date
/// is UTC.
fn strict_date_optional_time(text: &[u8], round_up: bool) -> Option<i64> {
    let mut c = Cursor { bytes: text, at: 0 };
    let year = c.digits(4)?;
    let (mut month, mut day) = (1, 1);
    let (mut hour, mut minute, mut second, mut nanos) = if round_up {
        (23, 59, 59, 999_999_999)
    } else {
        (0, 0, 0, 0)
    };
    if c.eat(b'-') {
        month = c.digits(2)?;
        if c.eat(b'-') {
            day = c.digits(2)?;
        }
    }
    if c.eat(b'T') {
        hour = c.digits(2)?;
        if c.eat(b':') {
            minute = c.digits(2)?;
            if c.eat(b':') {
                second = c.digits(2)?;
                if c.eat(b'.') || c.eat(b',') {
                    nanos = fraction(&mut c)?;
                }
            }
        }
    }
    let offset_seconds = zone(&mut c)?;
    if !c.done() || !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    // ARITH: every term is bounded (a 4-digit year, valid month, day and
    // time fields), so the sum fits far inside an i64 of milliseconds.
    #[allow(clippy::arithmetic_side_effects)]
    let millis = (days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
        - offset_seconds)
        * 1_000
        + nanos / 1_000_000;
    Some(millis)
}

/// One to nine digits of a second, as nanoseconds.
// ARITH: at most nine digits, so every value stays below 10^9.
#[allow(clippy::arithmetic_side_effects)]
fn fraction(c: &mut Cursor<'_>) -> Option<i64> {
    let mut nanos = 0i64;
    let mut n = 0;
    while let Some(d) = c.peek().filter(u8::is_ascii_digit) {
        if n == 9 {
            return None;
        }
        nanos = nanos * 10 + i64::from(d.wrapping_sub(b'0'));
        n += 1;
        c.at = c.at.saturating_add(1);
    }
    if n == 0 {
        return None;
    }
    Some(nanos * 10i64.pow(9 - n))
}

/// The zone, as seconds east of UTC; none is UTC.
fn zone(c: &mut Cursor<'_>) -> Option<i64> {
    if c.eat(b'Z') {
        return Some(0);
    }
    let sign = if c.eat(b'+') {
        1
    } else if c.eat(b'-') {
        -1
    } else {
        return Some(0);
    };
    let hours = c.digits(2)?;
    let colon = c.eat(b':');
    let minutes = if colon || c.peek().is_some_and(|b| b.is_ascii_digit()) {
        c.digits(2)?
    } else {
        0
    };
    if hours > 18 || minutes > 59 {
        return None;
    }
    // ARITH: at most 18 hours of seconds.
    #[allow(clippy::arithmetic_side_effects)]
    Some(sign * (hours * 3_600 + minutes * 60))
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`), for a year of at most four digits.
// ARITH: a year of 0..=9999 keeps every intermediate within a few million.
#[allow(clippy::arithmetic_side_effects)]
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Option<i64> {
        parse_to_millis(text, false).ok()
    }

    fn up(text: &str) -> Option<i64> {
        parse_to_millis(text, true).ok()
    }

    #[test]
    fn every_form_of_the_default_format_parses() {
        // The values the compat fixtures' documents hold.
        assert_eq!(at("1999-12-31"), Some(946_598_400_000));
        assert_eq!(at("2001-03-04"), Some(983_664_000_000));
        assert_eq!(at("2002-07-01T10:15:30Z"), Some(1_025_518_530_000));
        assert_eq!(at("2002-12-31T23:59:59.999Z"), Some(1_041_379_199_999));
        assert_eq!(at("1970"), Some(0));
        assert_eq!(at("1970-02"), Some(31 * 86_400_000));
        assert_eq!(at("1970-01-01T01"), Some(3_600_000));
        assert_eq!(at("1970-01-01T00:01"), Some(60_000));
        assert_eq!(at("1970-01-01T00:00:00,5"), Some(500));
        assert_eq!(at("1970-01-01T00:00:00.123456789"), Some(123));
        assert_eq!(at("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(at("1970-01-01T01:00:00+0100"), Some(0));
        assert_eq!(at("1970-01-01T00:00:00-01"), Some(3_600_000));
        assert_eq!(at("2000-02-29"), Some(951_782_400_000));
        assert_eq!(at("1969-12-31"), Some(-86_400_000));
    }

    #[test]
    fn round_up_fills_the_time_but_not_the_month_or_day() {
        assert_eq!(up("2002-12-31"), Some(1_041_379_199_999));
        assert_eq!(up("2002"), at("2002-01-01T23:59:59.999Z"));
        assert_eq!(up("2002-07-01T10"), at("2002-07-01T10:59:59.999Z"));
        assert_eq!(up("2002-07-01T10:15:30Z"), at("2002-07-01T10:15:30.999Z"));
        assert_eq!(up("2002-07-01T10:15:30.5Z"), at("2002-07-01T10:15:30.5Z"));
    }

    #[test]
    fn epoch_millis_is_the_second_parser() {
        assert_eq!(at("946598400000"), Some(946_598_400_000));
        assert_eq!(at("-1"), Some(-1));
        assert_eq!(at("12"), Some(12));
        assert_eq!(at("200"), Some(200), "three digits are not a year");
        assert_eq!(at("1.5"), Some(1));
        assert_eq!(at("-1.5"), Some(-2));
        assert_eq!(at("-1.0"), Some(-1));
        // Four digits are a year first.
        assert_eq!(at("2002"), Some(1_009_843_200_000));
    }

    #[test]
    fn bounds_round_as_range_queries_do() {
        assert_eq!(lower_bound("2002-07-01", true), Ok(1_025_481_600_000));
        assert_eq!(
            lower_bound("2002-07-01", false),
            Ok(1_025_481_600_000 + 86_400_000)
        );
        assert_eq!(upper_bound("2002-12-31", true), Ok(1_041_379_199_999));
        assert_eq!(upper_bound("2002-12-31", false), Ok(1_041_292_800_000 - 1));
    }

    #[test]
    fn malformed_dates_and_date_math_are_refused() {
        for bad in [
            "abc",
            "",
            "2002-13",
            "2002-02-30",
            "2001-02-29",
            "2002-1",
            "2002-01-01T24",
            "2002-01-01T10:60",
            "2002-01-01T10:00:61",
            "2002-01-01T",
            "2002-01-01T10:00:00.",
            "2002-01-01T10:00:00.1234567890",
            "2002-01-01Z1",
            "2002-01-01T10+19",
            "2002-01-01T10+01:60",
            "1.",
            "1.1234567",
            "1.x",
            "-",
            "x1",
            "99999999999999999999",
        ] {
            assert_eq!(
                parse_to_millis(bad, false),
                Err(DateError::Unparsable),
                "{bad}"
            );
        }
        assert_eq!(parse_to_millis("now-1d", false), Err(DateError::Math));
        assert_eq!(parse_to_millis("2002||+1d", false), Err(DateError::Math));
        assert_eq!(lower_bound("x", true), Err(DateError::Unparsable));
        assert_eq!(upper_bound("x", true), Err(DateError::Unparsable));
    }

    #[test]
    fn errors_are_opensearch_s() {
        let f = DateError::Unparsable.into_failure("abc");
        assert_eq!(f.kind, "parse_exception");
        assert!(f.opensearch);
        assert_eq!(
            f.reason,
            "failed to parse date field [abc] with format [strict_date_optional_time||epoch_millis]: \
             [failed to parse date field [abc] with format [strict_date_optional_time||epoch_millis]]"
        );
        let cause = f.cause.as_deref().expect("cause");
        assert_eq!(cause.kind, "illegal_argument_exception");
        assert_eq!(
            cause.cause.as_deref().map(|c| c.reason.as_str()),
            Some("Failed to parse with all enclosed parsers")
        );
        let math = DateError::Math.into_failure("now");
        assert_eq!(
            math.reason,
            "date math in [now] is not supported by this node yet"
        );
    }
}

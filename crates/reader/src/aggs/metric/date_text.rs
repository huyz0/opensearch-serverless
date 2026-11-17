//! A date field's value as text: `DocValueFormat.DateTime.format` with the
//! default format's printer, `DateFormatters.STRICT_DATE_OPTIONAL_TIME_PRINTER`
//! (server/src/main/java/org/opensearch/common/time/DateFormatters.java), in
//! UTC at millisecond resolution, at OpenSearch 3.10.0.
//!
//! The text is `yyyy-MM-dd'T'HH:mm:ss.SSSZ` with the proleptic Gregorian
//! calendar: the year at least four digits, signed when negative or past
//! 9999 (`SignStyle.EXCEEDS_PAD`). A double is cast to a long first, as
//! `format(double)` does.

use std::io::{self, Write};

const MILLIS_PER_DAY: i64 = 86_400_000;

/// The proleptic Gregorian `(year, month, day)` of `days` since 1970-01-01
/// (Howard Hinnant's `civil_from_days`, what `LocalDate.ofEpochDay` computes).
// ARITH: `days` is a long's milliseconds over a day, at most about 1.07e11
// in magnitude, so every step stays far inside an i64.
#[allow(clippy::arithmetic_side_effects)]
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        year,
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

/// Writes epoch milliseconds `millis` as the default date format prints them.
pub fn write_date(out: &mut dyn Write, millis: i64) -> io::Result<()> {
    let days = millis.div_euclid(MILLIS_PER_DAY);
    let in_day = millis.rem_euclid(MILLIS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    if year > 9999 {
        write!(out, "+{year}")?;
    } else if year < 0 {
        write!(out, "-{:04}", year.unsigned_abs())?;
    } else {
        write!(out, "{year:04}")?;
    }
    let hour = in_day / 3_600_000;
    let minute = in_day / 60_000 % 60;
    let second = in_day / 1_000 % 60;
    let milli = in_day % 1_000;
    write!(
        out,
        "-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{milli:03}Z"
    )
}

/// `format(double)`: `format((long) value)`, Java's cast truncating toward
/// zero and saturating (a NaN is 0).
pub fn write_date_of_double(out: &mut dyn Write, value: f64) -> io::Result<()> {
    // ARITH: none; `as` truncates and saturates exactly as Java's `(long)`.
    #[allow(clippy::cast_possible_truncation)]
    let millis = value as i64;
    write_date(out, millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(millis: i64) -> String {
        let mut out = Vec::new();
        write_date(&mut out, millis).expect("writes");
        String::from_utf8(out).expect("utf-8")
    }

    fn date_of(value: f64) -> String {
        let mut out = Vec::new();
        write_date_of_double(&mut out, value).expect("writes");
        String::from_utf8(out).expect("utf-8")
    }

    #[test]
    fn dates_print_in_the_default_format() {
        // Expected strings are what `DateFormatter.forPattern(
        // "strict_date_optional_time").format(...)` returns in UTC.
        assert_eq!(date(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(date(983_664_000_000), "2001-03-04T00:00:00.000Z");
        assert_eq!(date(1_025_518_530_000), "2002-07-01T10:15:30.000Z");
        assert_eq!(date(1_041_379_199_999), "2002-12-31T23:59:59.999Z");
        assert_eq!(date(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(date(-1), "1969-12-31T23:59:59.999Z");
        assert_eq!(date(-62_135_596_800_000), "0001-01-01T00:00:00.000Z");
        // The last moment of year 9999 still has four digits and no sign.
        assert_eq!(date(253_402_300_799_999), "9999-12-31T23:59:59.999Z");
        assert_eq!(date(-62_167_219_200_000), "0000-01-01T00:00:00.000Z");
        assert_eq!(date(-62_198_755_200_000), "-0001-01-01T00:00:00.000Z");
        assert_eq!(date(253_402_300_800_000), "+10000-01-01T00:00:00.000Z");
        assert_eq!(date(i64::MAX), "+292278994-08-17T07:12:55.807Z");
        assert_eq!(date(i64::MIN), "-292275055-05-16T16:47:04.192Z");
    }

    #[test]
    fn a_double_is_cast_as_java_casts_it() {
        assert_eq!(date_of(983_664_000_000.9), "2001-03-04T00:00:00.000Z");
        assert_eq!(date_of(-0.9), "1970-01-01T00:00:00.000Z");
        assert_eq!(date_of(f64::NAN), "1970-01-01T00:00:00.000Z");
        assert_eq!(date_of(f64::INFINITY), "+292278994-08-17T07:12:55.807Z");
    }
}

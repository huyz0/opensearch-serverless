//! HTTP dates (RFC 9110 `IMF-fixdate`, `Sun, 06 Nov 1994 08:49:37 GMT`), read
//! from the `Date` and `Last-Modified` headers of a HEAD so an object's age
//! comes from the store's clock alone.

// Every operand is a calendar field bounded by the checks in `parse` (a year
// of at most 9999, a day of at most 31, a time of day under 24 hours) or a day
// count derived from one; `format` clamps its input before dividing.
// ARITH: bounded as above, so no product nears i64's range.
#![allow(clippy::arithmetic_side_effects)]

#[cfg(any(feature = "emulator", test))]
const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Days from 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(any(feature = "emulator", test))]
/// The civil date of a day count from 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
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

/// Seconds since 1970-01-01 of an `IMF-fixdate`, or `None` if `text` is not one.
pub(crate) fn parse(text: &str) -> Option<u64> {
    let (_, rest) = text.split_once(", ")?;
    let mut parts = rest.split(' ');
    let day: i64 = parts.next()?.parse().ok()?;
    let name = parts.next()?;
    let month = i64::try_from(MONTHS.iter().position(|m| *m == name)?).ok()? + 1;
    let year: i64 = parts.next()?.parse().ok()?;
    let mut clock = parts.next()?.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next()?.parse().ok()?;
    if parts.next() != Some("GMT")
        || parts.next().is_some()
        || !(1..=31).contains(&day)
        || !(0..=9999).contains(&year)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

#[cfg(any(feature = "emulator", test))]
/// The `IMF-fixdate` of `secs` seconds since 1970-01-01.
pub(crate) fn format(secs: u64) -> String {
    let secs = i64::try_from(secs).unwrap_or(i64::MAX / 2);
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let weekday = usize::try_from(days.rem_euclid(7)).unwrap_or(0);
    // ALLOC: one date string, 29 bytes, per HEAD.
    format!(
        "{}, {day:02} {} {year:04} {:02}:{:02}:{:02} GMT",
        DAYS.get(weekday).copied().unwrap_or("Thu"),
        MONTHS
            .get(usize::try_from(month - 1).unwrap_or(0))
            .copied()
            .unwrap_or("Jan"),
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The RFC's own example and the epoch.
    #[test]
    fn the_rfc_example_and_the_epoch_round_trip() {
        assert_eq!(parse("Sun, 06 Nov 1994 08:49:37 GMT"), Some(784_111_777));
        assert_eq!(format(784_111_777), "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(parse("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(format(0), "Thu, 01 Jan 1970 00:00:00 GMT");
    }

    /// A leap day and a far date survive a round trip.
    #[test]
    fn leap_days_round_trip() {
        for secs in [951_782_400, 1_709_208_000, 4_102_444_799] {
            assert_eq!(parse(&format(secs)), Some(secs));
        }
        assert_eq!(format(1_709_208_000), "Thu, 29 Feb 2024 12:00:00 GMT");
    }

    /// Anything else is not a date.
    #[test]
    fn other_text_is_refused() {
        for bad in [
            "",
            "yesterday",
            "Sun, 06 Nov 1994 08:49:37 UTC",
            "Sun, 32 Nov 1994 08:49:37 GMT",
            "Sun, 06 Foo 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 25:49:37 GMT",
            "Sun, 06 Nov 99999999999999999 08:49:37 GMT",
            "Sun, 06 Nov 1994 08:49:37 GMT extra",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }
}

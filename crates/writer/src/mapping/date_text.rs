//! Reading a date's text in each format this writer reads, to
//! milliseconds since the epoch, with the Java's resolver checks.

/// Reads exactly `min..=max` ASCII digits from the front of `s`.
pub(super) fn digits(s: &mut &[u8], min: usize, max: usize) -> Option<i64> {
    let n = s
        .iter()
        .take(max)
        .take_while(|b| b.is_ascii_digit())
        .count();
    if n < min {
        return None;
    }
    let (head, rest) = s.split_at(n);
    *s = rest;
    head.iter().try_fold(0i64, |acc, b| {
        acc.checked_mul(10)?
            .checked_add(i64::from(b.wrapping_sub(b'0')))
    })
}

/// Consumes `c` from the front of `s` when it is there.
pub(super) fn eat(s: &mut &[u8], c: u8) -> bool {
    if s.first() == Some(&c) {
        *s = s.get(1..).unwrap_or_default();
        true
    } else {
        false
    }
}

/// Days in `month` of `year`, proleptic Gregorian.
pub(super) fn month_len(year: i64, month: i64) -> i64 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to a civil date (Hinnant's `days_from_civil`).
pub(super) fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    let y = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = y.div_euclid(400);
    let yoe = y.checked_sub(era.checked_mul(400)?)?;
    let mp = (month.checked_add(9)?) % 12;
    let doy = (153i64.checked_mul(mp)?.checked_add(2)? / 5)
        .checked_add(day)?
        .checked_sub(1)?;
    let doe = yoe
        .checked_mul(365)?
        .checked_add(yoe / 4)?
        .checked_sub(yoe / 100)?
        .checked_add(doy)?;
    era.checked_mul(146_097)?
        .checked_add(doe)?
        .checked_sub(719_468)
}

/// A validated civil date and time to epoch milliseconds.
#[allow(clippy::too_many_arguments)]
pub(super) fn to_millis(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    millis: i64,
    offset_minutes: i64,
) -> Option<i64> {
    if !(1..=12).contains(&month)
        || !(1..=month_len(year, month)).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    let secs = days
        .checked_mul(86_400)?
        .checked_add(hour.checked_mul(3_600)?)?
        .checked_add(minute.checked_mul(60)?)?
        .checked_add(second)?
        .checked_sub(offset_minutes.checked_mul(60)?)?;
    secs.checked_mul(1_000)?.checked_add(millis)
}

/// The first three digits of a 1-to-9-digit fraction, as milliseconds.
pub(super) fn fraction_millis(s: &mut &[u8], max: usize) -> Option<i64> {
    let n = s
        .iter()
        .take(max)
        .take_while(|b| b.is_ascii_digit())
        .count();
    if n == 0 {
        return None;
    }
    let mut ms = 0i64;
    for i in 0..3 {
        let d = s
            .get(i)
            .filter(|_| i < n)
            .map_or(0, |b| i64::from(b.wrapping_sub(b'0')));
        ms = ms.checked_mul(10)?.checked_add(d)?;
    }
    *s = s.get(n..).unwrap_or_default();
    Some(ms)
}

/// `Z`, `+HH`, `+HHmm` or `+HH:mm` (and `-`), in minutes east of UTC.
pub(super) fn offset(s: &mut &[u8]) -> Option<i64> {
    if eat(s, b'Z') {
        return Some(0);
    }
    let sign = if eat(s, b'+') {
        1
    } else if eat(s, b'-') {
        -1
    } else {
        return None;
    };
    let hours = digits(s, 2, 2)?;
    let colon = eat(s, b':');
    let minutes = if colon || s.first().is_some_and(u8::is_ascii_digit) {
        digits(s, 2, 2)?
    } else {
        0
    };
    // `ZoneOffset` holds at most 18 hours either way.
    if minutes > 59 || hours.saturating_mul(60).saturating_add(minutes) > 18 * 60 {
        return None;
    }
    // ARITH: hours * 60 + minutes is at most 1,080, so the result is too either way.
    #[allow(clippy::arithmetic_side_effects)]
    Some(sign * (hours * 60 + minutes))
}

/// `strict_date_optional_time` (`strict`) or `date_optional_time`:
/// `yyyy[-MM[-dd[THH[:mm[:ss[.S{1,9}]]][offset]]]]`, UTC when no offset.
pub(super) fn iso(text: &str, strict: bool) -> Option<i64> {
    let mut s = text.as_bytes();
    let (lo, two) = if strict { (4, 2) } else { (1, 1) };
    let year = digits(&mut s, lo, 4)?;
    let mut month = 1;
    let mut day = 1;
    let (mut hour, mut minute, mut second, mut millis, mut off) = (0, 0, 0, 0, 0);
    if eat(&mut s, b'-') {
        month = digits(&mut s, two, 2)?;
        if eat(&mut s, b'-') {
            day = digits(&mut s, two, 2)?;
            if eat(&mut s, b'T') {
                if let Some(h) = digits(&mut s, two, 2) {
                    hour = h;
                    if eat(&mut s, b':') {
                        minute = digits(&mut s, two, 2)?;
                        if eat(&mut s, b':') {
                            second = digits(&mut s, two, 2)?;
                            if eat(&mut s, b'.') || eat(&mut s, b',') {
                                millis = fraction_millis(&mut s, 9)?;
                            }
                        }
                    }
                }
                if !s.is_empty() {
                    off = offset(&mut s)?;
                }
            }
        }
    }
    if !s.is_empty() {
        return None;
    }
    to_millis(year, month, day, hour, minute, second, millis, off)
}

/// `epoch_millis` (`scale` 1, up to 6 fraction digits) or `epoch_second`
/// (`scale` 1000, up to 9): `[-]digits[.digits]`.
pub(super) fn epoch(text: &str, max_fraction: usize, scale: i64) -> Option<i64> {
    let mut s = text.as_bytes();
    let negative = eat(&mut s, b'-');
    let whole = digits(&mut s, 1, 19)?;
    let mut fraction = 0;
    if eat(&mut s, b'.') {
        let n = s.iter().take_while(|b| b.is_ascii_digit()).count();
        if n == 0 || n > max_fraction {
            return None;
        }
        if scale > 1 {
            fraction = fraction_millis(&mut s, max_fraction)?;
        } else {
            s = s.get(n..).unwrap_or_default();
        }
    }
    if !s.is_empty() {
        return None;
    }
    let v = whole.checked_mul(scale)?.checked_add(fraction)?;
    Some(if negative { v.checked_neg()? } else { v })
}

/// `yyyy/MM/dd` and, with `time`, `yyyy/MM/dd HH:mm:ss`, UTC.
pub(super) fn slash(text: &str, time: bool) -> Option<i64> {
    let mut s = text.as_bytes();
    let year = digits(&mut s, 4, 9)?;
    if !eat(&mut s, b'/') {
        return None;
    }
    let month = digits(&mut s, 2, 2)?;
    if !eat(&mut s, b'/') {
        return None;
    }
    let day = digits(&mut s, 2, 2)?;
    let (mut hour, mut minute, mut second) = (0, 0, 0);
    if time {
        if !eat(&mut s, b' ') {
            return None;
        }
        hour = digits(&mut s, 2, 2)?;
        if !eat(&mut s, b':') {
            return None;
        }
        minute = digits(&mut s, 2, 2)?;
        if !eat(&mut s, b':') {
            return None;
        }
        second = digits(&mut s, 2, 2)?;
    }
    if !s.is_empty() {
        return None;
    }
    to_millis(year, month, day, hour, minute, second, 0, 0)
}

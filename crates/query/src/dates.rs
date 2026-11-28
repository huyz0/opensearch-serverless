//! The plugin's timestamp literal strings as epoch milliseconds.

/// Milliseconds since the epoch of `yyyy-MM-dd`, `yyyy-MM-dd HH:mm:ss` or the same with
/// `.fff`, read as UTC; `None` for anything else (it is sent as written).
pub(crate) fn millis(s: &str) -> Option<i64> {
    let (date, time) = match s.split_once(' ') {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    if date.len() != 10 {
        return None;
    }
    let mut d = date.splitn(3, '-');
    let y: i64 = d.next()?.parse().ok()?;
    let mo: i64 = d.next()?.parse().ok().filter(|m| (1..=12).contains(m))?;
    let da: i64 = d.next()?.parse().ok().filter(|x| (1..=31).contains(x))?;
    let (mut h, mut mi, mut se, mut ms) = (0_i64, 0_i64, 0_i64, 0_i64);
    if let Some(t) = time {
        let (hms, frac) = t.split_once('.').map_or((t, None), |(a, b)| (a, Some(b)));
        let mut p = hms.splitn(3, ':');
        h = p.next()?.parse().ok().filter(|v| (0..24).contains(v))?;
        mi = p.next()?.parse().ok().filter(|v| (0..60).contains(v))?;
        se = p.next()?.parse().ok().filter(|v| (0..60).contains(v))?;
        if let Some(f) = frac {
            if f.is_empty() || f.len() > 3 || !f.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let digits: i64 = f.parse().ok()?;
            let scale = 10_i64.pow(3_u32.saturating_sub(u32::try_from(f.len()).ok()?));
            ms = digits.saturating_mul(scale);
        }
    }
    let days = days_from_civil(y, mo, da);
    Some(
        days.saturating_mul(86_400_000)
            .saturating_add(h.saturating_mul(3_600_000))
            .saturating_add(mi.saturating_mul(60_000))
            .saturating_add(se.saturating_mul(1000))
            .saturating_add(ms),
    )
}

/// Days from 1970-01-01 to the proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y.saturating_sub(1) } else { y };
    let era = y.div_euclid(400);
    let yoe = y.saturating_sub(era.saturating_mul(400));
    let mp = if m > 2 {
        m.saturating_sub(3)
    } else {
        m.saturating_add(9)
    };
    let doy =
        (153_i64.saturating_mul(mp).saturating_add(2) / 5).saturating_add(d.saturating_sub(1));
    let doe = yoe
        .saturating_mul(365)
        .saturating_add(yoe / 4)
        .saturating_sub(yoe / 100)
        .saturating_add(doy);
    era.saturating_mul(146_097)
        .saturating_add(doe)
        .saturating_sub(719_468)
}

#[cfg(test)]
mod tests {
    use super::millis;

    #[test]
    fn dates_and_times_read_as_utc_milliseconds() {
        assert_eq!(millis("1970-01-01"), Some(0));
        assert_eq!(millis("2024-01-01 00:00:00"), Some(1_704_067_200_000));
        assert_eq!(millis("2024-01-31 00:00:00"), Some(1_706_659_200_000));
        assert_eq!(millis("2024-01-01 00:00:01.5"), Some(1_704_067_201_500));
        assert_eq!(millis("1969-12-31 23:59:59"), Some(-1000));
    }

    #[test]
    fn anything_else_is_left_alone() {
        for s in [
            "",
            "x",
            "2024-13-01",
            "2024-01-01T00:00:00Z",
            "2024-01-01 25:00:00",
            "2024-01-01 00:00:00.",
            "2024-1-1",
        ] {
            assert_eq!(millis(s), None, "{s}");
        }
    }
}

//! A `time_zone`'s id: a fixed offset as `ZoneId.of` reads it, or an IANA
//! region zone, at OpenSearch 3.10.0.

use crate::aggs::bucket::date_histogram::RegionZone;

/// A fixed-offset zone's offset in milliseconds, for the ids `ZoneId.of`
/// reads as one: `Z`, `UTC`, `GMT`, `UT`, and `+h`, `+hh`, `+hh:mm`,
/// `+hhmm`, `+hh:mm:ss` or `+hhmmss`, alone or after `UTC`, `GMT` or `UT`.
pub(crate) fn zone_millis(id: &str) -> Option<i64> {
    if matches!(id, "Z" | "UTC" | "GMT" | "UT") {
        return Some(0);
    }
    let rest = ["UTC", "GMT", "UT"]
        .iter()
        .find_map(|p| id.strip_prefix(p))
        .unwrap_or(id);
    let sign = match rest.as_bytes().first()? {
        b'+' => 1i64,
        b'-' => -1i64,
        _ => return None,
    };
    let body = rest.get(1..)?;
    // ALLOC: at most three short slices.
    let parts: Vec<&str> = if body.contains(':') {
        body.split(':').collect()
    } else if body.len() <= 2 {
        // ALLOC: one slice.
        vec![body]
    } else if body.len() == 4 || body.len() == 6 {
        body.as_bytes()
            .chunks(2)
            .map(|c| std::str::from_utf8(c).unwrap_or(""))
            // ALLOC: at most three short slices.
            .collect()
    } else {
        return None;
    };
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut seconds = 0i64;
    for (i, part) in parts.iter().enumerate() {
        let ok_len = if i == 0 {
            (1..=2).contains(&part.len())
        } else {
            part.len() == 2
        };
        if !ok_len || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n: i64 = part.parse().ok()?;
        if i > 0 && n > 59 {
            return None;
        }
        seconds = seconds.saturating_add(n.saturating_mul([3600, 60, 1][i]));
    }
    if seconds > 18 * 3600 {
        return None;
    }
    Some(sign.saturating_mul(seconds).saturating_mul(1000))
}

/// A zone id as `(fixed offset in milliseconds, region zone)`: a fixed
/// offset has no region, and a region has a zero offset; `None` for an id
/// that is neither.
pub(crate) fn zone_of(id: &str) -> Option<(i64, Option<RegionZone>)> {
    zone_millis(id)
        .map(|fixed| (fixed, None))
        .or_else(|| RegionZone::named(id).map(|named| (0, Some(named))))
}

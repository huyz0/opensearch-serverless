//! A keep-alive: the time value a point in time is asked to be held for.

use oss_rest::RestError;

/// How long a view is held when the caller does not say.
#[cfg(feature = "writer")]
pub(crate) const DEFAULT_KEEP_ALIVE_MILLIS: u64 = 300_000;

/// The longest a view may be held, so one request cannot pin a commit for a
/// week: refused, not clamped.
pub(crate) const MAX_KEEP_ALIVE_MILLIS: u64 = 3_600_000;

/// A time value as `TimeValue.parseTimeValue` reads it for the units a
/// keep-alive uses: whole numbers of `nanos`, `micros`, `ms`, `s`, `m`, `h`
/// or `d`, and a bare `0`. Milliseconds, rounded down.
pub(crate) fn time_millis(name: &str, value: &str) -> Result<u64, RestError> {
    let bad = |why: &str| {
        // ALLOC: error path; the message names the value.
        RestError::illegal_argument(format!(
            "failed to parse setting [{name}] with value [{value}] as a time value: {why}"
        ))
    };
    if value == "0" {
        return Ok(0);
    }
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| bad("unit is missing or unrecognized"))?;
    let (digits, unit) = value.split_at(split);
    let number: u64 = digits
        .parse()
        .map_err(|_| bad("not a whole number of units"))?;
    let (times, per) = match unit {
        "nanos" => (1, 1_000_000),
        "micros" => (1, 1_000),
        "ms" => (1, 1),
        "s" => (1_000, 1),
        "m" => (60_000, 1),
        "h" => (3_600_000, 1),
        "d" => (86_400_000, 1),
        _ => return Err(bad("unit is missing or unrecognized")),
    };
    number
        .checked_mul(times)
        .and_then(|n| n.checked_div(per))
        .ok_or_else(|| bad("the value is too large"))
}

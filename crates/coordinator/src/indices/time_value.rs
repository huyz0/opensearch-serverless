//! A time value as a rollover condition reads one: `TimeValue.parseTimeValue`
//! and `toHumanReadableString(1)`.

/// A duration, kept as a count of a unit as the Java keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeValue {
    duration: i64,
    unit_nanos: i64,
}

const NANOS: i64 = 1;
const MICROS: i64 = 1_000;
const MILLIS: i64 = 1_000_000;
const SECONDS: i64 = 1_000_000_000;
const MINUTES: i64 = 60 * SECONDS;
const HOURS: i64 = 60 * MINUTES;
const DAYS: i64 = 24 * HOURS;

impl TimeValue {
    /// The duration in nanoseconds, saturating as `TimeUnit.toNanos` does.
    pub fn nanos(&self) -> i64 {
        self.duration.saturating_mul(self.unit_nanos)
    }

    /// The duration in milliseconds, truncated toward zero.
    pub fn millis(&self) -> i64 {
        self.nanos() / MILLIS
    }

    /// The value as `toHumanReadableString(1)` prints it: the largest unit it
    /// reaches, with one fraction digit cut (not rounded) and dropped when it
    /// is zero.
    pub fn human(&self) -> String {
        if self.duration < 0 {
            // ALLOC: one short string or list, bounded by the request body.
            return self.duration.to_string();
        }
        let nanos = self.nanos();
        if nanos == 0 {
            // ALLOC: one short string or list, bounded by the request body.
            return "0s".to_owned();
        }
        let (unit, suffix) = [
            (DAYS, "d"),
            (HOURS, "h"),
            (MINUTES, "m"),
            (SECONDS, "s"),
            (MILLIS, "ms"),
            (MICROS, "micros"),
        ]
        .into_iter()
        .find(|(unit, _)| nanos >= *unit)
        .unwrap_or((NANOS, "nanos"));
        // ALLOC: one short string.
        format!("{}{suffix}", one_fraction_digit(nanos as f64 / unit as f64))
    }
}

/// `formatDecimal(value, 1)`: the integral part, then the first fraction digit
/// unless it is zero.
fn one_fraction_digit(value: f64) -> String {
    // ALLOC: one short string.
    let text = format!("{value:?}");
    let Some((whole, fraction)) = text.split_once('.') else {
        return text;
    };
    match fraction.chars().next() {
        // ALLOC: one short string or list, bounded by the request body.
        Some(digit) if digit != '0' => format!("{whole}.{digit}"),
        // ALLOC: one short string or list, bounded by the request body.
        _ => whole.to_owned(),
    }
}

/// Reads `text` as `TimeValue.parseTimeValue(text, setting)` does.
///
/// # Errors
///
/// The Java's message for a missing or unrecognised unit, a negative
/// duration, a fractional one and text that is not a number.
pub fn parse(text: &str, setting: &str) -> Result<TimeValue, String> {
    // ALLOC: one lower-cased copy of a short value.
    let normalized = text.to_lowercase();
    let normalized = normalized.trim();
    let unit = if normalized.ends_with("nanos") {
        Some(("nanos", NANOS))
    } else if normalized.ends_with("micros") {
        Some(("micros", MICROS))
    } else if normalized.ends_with("ms") {
        Some(("ms", MILLIS))
    } else if normalized.ends_with('s') {
        Some(("s", SECONDS))
    } else if text.ends_with('m') {
        // Minutes are case-sensitive: `M` means months, which is not a unit here.
        Some(("m", MINUTES))
    } else if normalized.ends_with('h') {
        Some(("h", HOURS))
    } else if normalized.ends_with('d') {
        Some(("d", DAYS))
    } else {
        None
    };
    if let Some((suffix, unit_nanos)) = unit {
        let number = normalized.strip_suffix(suffix).unwrap_or(normalized).trim();
        let duration = whole_number(text, number, setting)?;
        return Ok(TimeValue {
            duration,
            unit_nanos,
        });
    }
    let digits = normalized.trim_start_matches('-');
    let zeros = |s: &str| !s.is_empty() && s.bytes().all(|b| b == b'0');
    let minus_one = normalized.starts_with('-')
        && digits
            .strip_suffix('1')
            .is_some_and(|z| z.bytes().all(|b| b == b'0'));
    if minus_one {
        return Ok(TimeValue {
            duration: -1,
            unit_nanos: NANOS,
        });
    }
    if zeros(normalized) {
        return Ok(TimeValue {
            duration: 0,
            unit_nanos: NANOS,
        });
    }
    // ALLOC: one short string or list, bounded by the request body.
    Err(format!(
        "failed to parse setting [{setting}] with value [{text}] as a time value: unit is missing or unrecognized"
    ))
}

fn whole_number(input: &str, number: &str, setting: &str) -> Result<i64, String> {
    match number.parse::<i64>() {
        // ALLOC: one short string or list, bounded by the request body.
        Ok(value) if value < -1 => Err(format!(
            "failed to parse setting [{setting}] with value [{input}] as a time value: negative durations are not supported"
        )),
        Ok(value) => Ok(value),
        // ALLOC: one short string or list, bounded by the request body.
        Err(_) if number.parse::<f64>().is_ok() => Err(format!(
            "failed to parse [{input}], fractional time values are not supported"
        )),
        // ALLOC: one short string or list, bounded by the request body.
        Err(_) => Err(format!("failed to parse [{input}]")),
    }
}

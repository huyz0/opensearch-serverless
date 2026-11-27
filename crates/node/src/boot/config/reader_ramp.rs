//! `OSS_READER_RAMP_MAX_SECS` and `OSS_READER_RAMP_MIN_SECS`: a joining
//! reader's slow start (docs/specs/reader-slow-start). Off unless the
//! maximum is set.

/// The bounds of the slow start, seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Secs {
    /// By when a joining reader is fully ramped, whatever it says; 0 is off.
    pub max: i64,
    /// The soonest it may be fully ramped, however warm it says it is.
    pub min: i64,
}

fn secs(get: &impl Fn(&str) -> Option<String>, key: &str, default: i64) -> Result<i64, String> {
    let Some(text) = get(key).filter(|v| !v.is_empty()) else {
        return Ok(default);
    };
    let n: i64 = text.parse().map_err(|e| format!("{key}: {e}"))?;
    if (0..=86_400).contains(&n) {
        Ok(n)
    } else {
        Err(format!("{key} [{n}] is outside 0 to 86400"))
    }
}

pub(super) fn read(get: &impl Fn(&str) -> Option<String>) -> Result<Secs, String> {
    let max = secs(get, "OSS_READER_RAMP_MAX_SECS", 0)?;
    let min = secs(get, "OSS_READER_RAMP_MIN_SECS", 30)?;
    if max > 0 && min > max {
        return Err(format!(
            "OSS_READER_RAMP_MIN_SECS [{min}] is above OSS_READER_RAMP_MAX_SECS [{max}]"
        ));
    }
    Ok(Secs { max, min })
}

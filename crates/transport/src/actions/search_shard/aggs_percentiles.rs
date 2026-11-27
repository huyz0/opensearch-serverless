//! The `percentiles` and `percentile_ranks` builders of a forwarded search:
//! the fields they write after the values-source fields.

use super::Refusal;
use crate::error::WireError;
use crate::stream::Input;

/// What a `percentiles` or `percentile_ranks` asks beyond its field.
#[derive(Debug, Clone, PartialEq)]
pub struct PercentilesRequest {
    /// The percents or the values ranked, as the builder holds them.
    pub keys: Vec<f64>,
    /// `keyed`.
    pub keyed: bool,
    /// The t-digest's compression; the default when none.
    pub compression: Option<f64>,
}

/// The most keys a percentiles aggregation may carry.
const MAX_KEYS: usize = 1024;

pub(super) fn double(input: &mut Input<'_>, what: &'static str) -> Result<f64, WireError> {
    Ok(f64::from_bits(input.long(what)?.cast_unsigned()))
}

/// A percentiles builder's own fields: the keys, `keyed`, and the optional
/// `PercentilesConfig` (a method byte, 0 for the t-digest, and its
/// compression).
pub fn read(input: &mut Input<'_>) -> Result<PercentilesRequest, Refusal> {
    let n = input.count(8, "percentile keys")?;
    if n > MAX_KEYS {
        return Err(Refusal::TooLarge("percentile keys"));
    }
    // ALLOC: one double a key, at most MAX_KEYS.
    let mut keys = Vec::with_capacity(n);
    for _ in 0..n {
        keys.push(double(input, "a percentile key")?);
    }
    let keyed = input.bool("keyed")?;
    let mut compression = None;
    if input.bool("a percentiles config")? {
        if input.byte("a percentiles method")? != 0 {
            return Err(Refusal::Unsupported(
                "a percentiles method other than tdigest",
            ));
        }
        compression = Some(double(input, "a compression")?);
    }
    Ok(PercentilesRequest {
        keys,
        keyed,
        compression,
    })
}

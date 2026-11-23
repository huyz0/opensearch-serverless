//! A stripe's open-bucket register: which generation of the stripe's buckets
//! takes new members.
//!
//! Ported from `BucketStore` (`OPEN_ROOT`, `readOpen`, `swapOpen`) on
//! huyz0/OpenSearch `feature/serverlessshell`. The register
//! `bucket-open/<stripe>` holds the generation as decimal ASCII.

use crate::error::{FormatError, corrupt};
use crate::json_write::push_u64;

const WHAT: &str = "open bucket";

/// Where under a deployment's base the stripes' open-bucket registers are
/// (`BucketStore.OPEN_ROOT`).
pub const BUCKET_OPEN_PREFIX: &str = "bucket-open/";

/// The key of a stripe's open-bucket register: `bucket-open/<stripe>`.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base that is not empty and does not end in
/// `/`.
pub fn bucket_open_key(base: &str, stripe: usize) -> Result<String, FormatError> {
    if !base.is_empty() && !base.ends_with('/') {
        return Err(corrupt("blob key", "a base is empty or ends in a slash"));
    }
    // ALLOC: one key, the length of its parts.
    Ok(format!("{base}{BUCKET_OPEN_PREFIX}{stripe}"))
}

/// The register's value for a bucket generation: its decimal digits.
#[must_use]
pub fn encode_open(bucket_generation: u64) -> Vec<u8> {
    // ALLOC: at most twenty digits, once a swap.
    let mut out = Vec::with_capacity(20);
    push_u64(&mut out, bucket_generation);
    out
}

/// The bucket generation a register's value names: decimal digits, with
/// surrounding whitespace allowed, no sign, and within `i64` (the shell's
/// `Long.parseLong(value.trim())`, for a generation that is never negative).
///
/// # Errors
///
/// [`FormatError::Corrupt`] for anything else.
pub fn parse_open(value: &[u8]) -> Result<u64, FormatError> {
    let start = value.iter().position(|b| *b > b' ').unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|b| *b > b' ')
        .map_or(start, |i| i.saturating_add(1));
    let digits = value.get(start..end).unwrap_or_default();
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(corrupt(WHAT, "a generation is decimal digits"));
    }
    let text = std::str::from_utf8(digits).map_err(|_| corrupt(WHAT, "a generation is digits"))?;
    text.parse::<i64>()
        .map(i64::unsigned_abs)
        .map_err(|_| corrupt(WHAT, "a generation is past a signed 64-bit integer"))
}

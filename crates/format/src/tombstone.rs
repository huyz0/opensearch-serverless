//! The tombstone a deleted index leaves in its name's register, and the
//! hourly bucket its marker is filed under.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/Tombstones.java`.
//! A tombstone keeps a deleted name's register, and so its generation, until
//! a sweep removes it: a name created again swaps over it, so a writer still
//! holding a generation from before the delete cannot land on the new index.

use crate::descriptor::TOMBSTONE_PREFIX;
use crate::error::{FormatError, corrupt};
use crate::json::out::Out;
use crate::json::{Reader, Token};

const WHAT: &str = "tombstone";

/// The longest tombstone: the prefix, two fields and two `i64`s.
pub const MAX_TOMBSTONE_BYTES: usize = 86;

/// A deleted index's tombstone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tombstone {
    /// When the delete happened, epoch milliseconds; 0 for a tombstone
    /// written before that was recorded.
    pub deleted_at: i64,
    /// When a sweep claimed it for removal, or 0 if none has.
    pub reaping_at: i64,
}

impl Tombstone {
    /// Writes the tombstone as the Java's `Tombstones.tombstone`, into the
    /// front of `buf`, and returns its length. `reaping_at` is written only
    /// when it is not 0.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] when `buf` is shorter than the tombstone;
    /// [`MAX_TOMBSTONE_BYTES`] always suffices.
    pub fn write(&self, buf: &mut [u8]) -> Result<usize, FormatError> {
        let mut out = Out::new(buf);
        out.raw(TOMBSTONE_PREFIX);
        out.raw(b",");
        // The prefix opened the object and wrote its first field.
        out.int_field("deleted_at", self.deleted_at);
        if self.reaping_at != 0 {
            out.int_field("reaping_at", self.reaping_at);
        }
        out.raw(b"}");
        out.finish(WHAT)
    }

    /// Parses a tombstone: an object starting [`TOMBSTONE_PREFIX`], whose
    /// integer `deleted_at` and `reaping_at` are read when present and every
    /// other field skipped.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for bytes without the prefix, malformed JSON,
    /// a repeated time, or a time that is not an integer (the Java reads a
    /// string as 0 and truncates a fraction).
    pub fn parse(value: &[u8]) -> Result<Self, FormatError> {
        if !value.starts_with(TOMBSTONE_PREFIX) {
            return Err(corrupt(WHAT, "not a tombstone"));
        }
        let mut json = Reader::new(value, WHAT);
        json.start_object("a tombstone is an object")?;
        let mut deleted_at = None;
        let mut reaping_at = None;
        while let Some(field) = json.next_field()? {
            let slot = match field.as_ref() {
                "deleted_at" => &mut deleted_at,
                "reaping_at" => &mut reaping_at,
                _ => {
                    json.skip_value()?;
                    continue;
                }
            };
            let Token::Int(n) = json.expect_token()? else {
                return Err(json.error("a tombstone's times are integers"));
            };
            if slot.replace(n).is_some() {
                return Err(json.error("a tombstone's time is repeated"));
            }
        }
        json.finish()?;
        Ok(Self {
            deleted_at: deleted_at.unwrap_or(0),
            reaping_at: reaping_at.unwrap_or(0),
        })
    }
}

/// The bucket a delete at `millis` files its marker under: the UTC hour it
/// falls in, as `uuuuMMddHH` (`Tombstones.tombstoneBucket`).
///
/// # Errors
///
/// [`FormatError::Corrupt`] for an instant before 1970 or after 9999, where
/// the Java's pattern would write a sign or a fifth digit.
pub fn tombstone_bucket(millis: i64) -> Result<[u8; 10], FormatError> {
    // The first millisecond of the year 10000.
    const END: i64 = 253_402_300_800_000;
    if !(0..END).contains(&millis) {
        return Err(corrupt(WHAT, "a delete time must be from 1970 to 9999"));
    }
    let hours = millis.div_euclid(3_600_000);
    let (year, month, day) = civil(hours.div_euclid(24));
    Ok(bucket_name(year, month, day, hours.rem_euclid(24)))
}

/// `uuuuMMddHH` of a year from 1970 to 9999 and a valid month, day and hour.
// ARITH: each value is non-negative and fits its width, so every digit is
// 0 to 9 and every index is below 10.
#[allow(clippy::arithmetic_side_effects)]
fn bucket_name(year: i64, month: i64, day: i64, hour: i64) -> [u8; 10] {
    let mut name = [0u8; 10];
    let mut end = 0;
    for (value, width) in [(year, 4), (month, 2), (day, 2), (hour, 2)] {
        end += width;
        let mut rest = value;
        for at in (end - width..end).rev() {
            name[at] = b'0' + (rest % 10) as u8;
            rest /= 10;
        }
    }
    name
}

/// The civil date of a day counted from 1970-01-01, for days from 0 to the
/// end of 9999 (Howard Hinnant's `civil_from_days`).
// ARITH: days is in [0, 2,932,897), so every intermediate is a small
// non-negative number far inside an i64.
#[allow(clippy::arithmetic_side_effects)]
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

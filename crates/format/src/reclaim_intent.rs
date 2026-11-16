//! What a delete still owes: the intent it files before removing an index's
//! descriptor, and the minute bucket the intent falls due in.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/ReclaimQueue.java`
//! (`Intent`, `enqueue`'s body, `parse`, `BUCKET` and `bucketStart`). The
//! Java writes the body by concatenation, so its bytes are exactly
//! `{"name":"<name>","uuid":"<uuid>","shards":<n>,"had_writer":<bool>}`;
//! names and uuids hold nothing JSON escapes, and [`ReclaimIntent::write`]
//! escapes anyway.

use std::borrow::Cow;

use crate::error::{FormatError, corrupt};
use crate::index_record::MAX_SHARDS;
use crate::json::out::Out;
use crate::json::{Reader, Token};
use crate::tombstone::{civil, days_from_civil, days_in_month};

const WHAT: &str = "reclaim intent";

/// The longest intent a reader takes: a name and a uuid of a key segment
/// each, escaped, with room to spare. A longer one was not written by a
/// queue.
pub const MAX_RECLAIM_INTENT_BYTES: usize = 4096;

/// One deleted index and what finishing its delete takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimIntent<'a> {
    /// The index's name.
    pub name: Cow<'a, str>,
    /// The deleted incarnation's uuid.
    pub uuid: Cow<'a, str>,
    /// How many shards it had.
    pub shards: u32,
    /// Whether any shard had an owner at the delete: then its bytes are
    /// purged when the intent falls due rather than at the delete.
    pub had_writer: bool,
}

impl ReclaimIntent<'_> {
    /// Writes the intent as the Java's `enqueue` does, into the front of
    /// `buf`, and returns its length.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] when it does not fit `buf`.
    pub fn write(&self, buf: &mut [u8]) -> Result<usize, FormatError> {
        let mut out = Out::new(buf);
        out.start_object();
        out.string_field("name", &self.name);
        out.string_field("uuid", &self.uuid);
        out.int_field("shards", i64::from(self.shards));
        out.bool_field("had_writer", self.had_writer);
        out.end_object();
        out.finish(WHAT)
    }
}

impl<'a> ReclaimIntent<'a> {
    /// Parses an intent as the Java's `parse`: string `name` and `uuid`, a
    /// number `shards`, and `had_writer` false only when it is the literal
    /// `false` (absent or anything else reads as true: purging again is
    /// only a cost, skipping it could leak). Other fields are skipped.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for malformed JSON, a repeated field, a
    /// missing or mistyped `name`, `uuid` or `shards`, or a shard count
    /// outside 0 to [`MAX_SHARDS`]. The Java would take a fraction or a
    /// count past that; no queue writes one, so it is refused rather than
    /// reclaimed over an unbounded count.
    pub fn parse(value: &'a [u8]) -> Result<Self, FormatError> {
        let mut json = Reader::new(value, WHAT);
        json.start_object("a reclaim intent is an object")?;
        let (mut name, mut uuid, mut shards, mut had_writer) = (None, None, None, None);
        while let Some(field) = json.next_field()? {
            let repeated = match field.as_ref() {
                "name" => name.replace(string(&mut json)?).is_some(),
                "uuid" => uuid.replace(string(&mut json)?).is_some(),
                "shards" => {
                    let Token::Int(n) = json.expect_token()? else {
                        return Err(json.error("a reclaim intent's shards is an integer"));
                    };
                    let n = u32::try_from(n)
                        .ok()
                        .filter(|n| *n <= MAX_SHARDS)
                        .ok_or_else(|| json.error("a reclaim intent's shards is 0 to 4096"))?;
                    shards.replace(n).is_some()
                }
                "had_writer" => {
                    let first = json.expect_token()?;
                    let value = first != Token::Bool(false);
                    json.skip_after(&first)?;
                    had_writer.replace(value).is_some()
                }
                _ => {
                    json.skip_value()?;
                    false
                }
            };
            if repeated {
                return Err(json.error("a reclaim intent's field is repeated"));
            }
        }
        json.finish()?;
        match (name, uuid, shards) {
            (Some(name), Some(uuid), Some(shards)) => Ok(Self {
                name,
                uuid,
                shards,
                had_writer: had_writer.unwrap_or(true),
            }),
            _ => Err(corrupt(
                WHAT,
                "a reclaim intent names an index, a uuid and shards",
            )),
        }
    }
}

/// A string value, or a corrupt intent.
fn string<'a>(json: &mut Reader<'a>) -> Result<Cow<'a, str>, FormatError> {
    match json.expect_token()? {
        Token::Str(text) => Ok(text),
        _ => Err(json.error("a reclaim intent's name and uuid are strings")),
    }
}

/// The bucket an intent due at `millis` is filed under: the UTC minute it
/// falls in, as `uuuuMMddHHmm` (the Java's `BUCKET`).
///
/// # Errors
///
/// [`FormatError::Corrupt`] for an instant before 1970 or after 9999, where
/// the Java's pattern would write a sign or a fifth digit.
pub fn reclaim_bucket(millis: i64) -> Result<[u8; 12], FormatError> {
    // The first millisecond of the year 10000.
    const END: i64 = 253_402_300_800_000;
    if !(0..END).contains(&millis) {
        return Err(corrupt(WHAT, "a due time must be from 1970 to 9999"));
    }
    let minutes = millis.div_euclid(60_000);
    let hours = minutes.div_euclid(60);
    let (year, month, day) = civil(hours.div_euclid(24));
    Ok(bucket_name([
        (year, 4),
        (month, 2),
        (day, 2),
        (hours.rem_euclid(24), 2),
        (minutes.rem_euclid(60), 2),
    ]))
}

/// The digits of each value, at its width.
// ARITH: each value is non-negative and fits its width, so every digit is
// 0 to 9 and every index is below 12.
#[allow(clippy::arithmetic_side_effects)]
fn bucket_name(fields: [(i64, usize); 5]) -> [u8; 12] {
    let mut name = [0u8; 12];
    let mut end = 0;
    for (value, width) in fields {
        end += width;
        let mut rest = value;
        for at in (end - width..end).rev() {
            name[at] = b'0' + (rest % 10) as u8;
            rest /= 10;
        }
    }
    name
}

/// When the bucket named `name` starts, epoch milliseconds: the inverse of
/// [`reclaim_bucket`] (the Java's `bucketStart`). `None` for a name that is
/// not twelve digits naming a real UTC minute from 1970 on, which the Java
/// answers -1 and its drain skips.
pub fn reclaim_bucket_start(name: &str) -> Option<i64> {
    let digits = name.as_bytes();
    if digits.len() != 12 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let field = |from: usize, to: usize| -> i64 {
        digits.get(from..to).map_or(0, |part| {
            part.iter().fold(0i64, |n, d| {
                n.saturating_mul(10)
                    .saturating_add(i64::from(d.saturating_sub(b'0')))
            })
        })
    };
    let (year, month, day) = (field(0, 4), field(4, 6), field(6, 8));
    let (hour, minute) = (field(8, 10), field(10, 12));
    if year < 1970 || !(1..=12).contains(&month) || hour > 23 || minute > 59 {
        return None;
    }
    if day < 1 || day > days_in_month(year, month) {
        return None;
    }
    Some(minute_start(
        days_from_civil(year, month, day),
        hour,
        minute,
    ))
}

/// Milliseconds at `hour:minute` of day `days` from the epoch.
// ARITH: days is under 2,932,897, hour under 24 and minute under 60, so the
// product is far inside an i64.
#[allow(clippy::arithmetic_side_effects)]
fn minute_start(days: i64, hour: i64, minute: i64) -> i64 {
    ((days * 24 + hour) * 60 + minute) * 60_000
}

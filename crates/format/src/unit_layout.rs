//! An index's unit layout: the shard mode, the declared shard count, and
//! the units that split the 64-bit routing hash space between them.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/routing/UnitLayoutFormat.java`
//! and `ShardMode.java`. The Java writes compact JSON with `XContentBuilder`:
//! `mode`, `declared_shards`, `next_unit_id`, then `units` in hash order,
//! each `{"id","first","last"}` with the hashes as unsigned lower-case hex
//! without leading zeros (`Long.toHexString`), and `pending_split` (the
//! retired unit and its two halves) only while a split is pending. Unknown
//! fields are skipped, for a layout written by a newer node.
//!
//! This is the bytes only: whether the units cover the hash space and keep
//! the shape rule is checked where the layout is used (`oss-cluster`'s
//! `routing`). Where the Java's parser coerces a JSON type it never writes
//! there (a number as text, `1.0` for `1`, a hash as a number), this
//! refuses (`fixtures/routing/refusals.txt`).

use crate::error::FormatError;
use crate::index_record::MAX_SHARDS;
use crate::json::out::Out;
use crate::json::{Reader, Token};

const WHAT: &str = "unit layout";

/// The longest layout read or written: [`MAX_SHARDS`] units of the longest
/// form, a pending split and the header fields.
pub const MAX_UNIT_LAYOUT_BYTES: usize = 288 * 1024;

/// How an index's documents map to its physical shards (`ShardMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShardMode {
    /// One unit per declared shard from creation, never cut automatically.
    Fixed,
    /// One unit covering every declared shard at creation, cut as it grows.
    Adaptive,
}

impl ShardMode {
    /// The setting key that asks for a mode at index creation; it is kept
    /// in the descriptor, never in the index settings.
    pub const SETTING: &'static str = "index.serverless.shard_mode";

    /// The mode's name as settings and the descriptor write it.
    pub fn setting_value(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Adaptive => "adaptive",
        }
    }

    /// The mode named by `value`, as `ShardMode.parse` reads it: Java's
    /// `trim` (every character at or below a space), then case ignored.
    /// `None` when it names neither mode.
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim_matches(|c: char| c <= ' ');
        [Self::Fixed, Self::Adaptive]
            .into_iter()
            .find(|mode| trimmed.eq_ignore_ascii_case(mode.setting_value()))
    }
}

/// One unit and the hash range it covers, both ends inclusive and unsigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnitRecord {
    /// The unit id, unique for the index's life; never negative once read.
    pub id: i32,
    /// The first hash it covers.
    pub first: u64,
    /// The last hash it covers.
    pub last: u64,
}

/// A unit layout as the Java's `UnitLayoutFormat` writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitLayoutRecord {
    /// The shard mode.
    pub mode: ShardMode,
    /// The user's `number_of_shards`.
    pub declared_shards: i32,
    /// The id the next new unit gets.
    pub next_unit_id: i32,
    /// The live units, in the order written (hash order when the Java wrote
    /// them).
    pub units: Vec<UnitRecord>,
    /// A planned split: the retired unit, then its lower and upper halves.
    pub pending: Option<[UnitRecord; 3]>,
}

impl UnitLayoutRecord {
    /// Reads a layout from its JSON object bytes.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] past [`MAX_UNIT_LAYOUT_BYTES`] or
    /// [`MAX_SHARDS`] units; [`FormatError::Corrupt`] when it is not a JSON
    /// object, a required field is missing or of another type, a field is
    /// repeated, a hash is not hex that fits 64 bits, a count is not
    /// positive, or a pending split does not name three units.
    pub fn parse(value: &[u8]) -> Result<Self, FormatError> {
        if value.len() > MAX_UNIT_LAYOUT_BYTES {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: MAX_UNIT_LAYOUT_BYTES as u64,
                actual: u64::try_from(value.len()).unwrap_or(u64::MAX),
            });
        }
        let mut json = Reader::new(value, WHAT);
        json.start_object("a unit layout is an object")?;
        let mut mode = None;
        let mut declared = None;
        let mut next = None;
        let mut units = None;
        let mut pending = None;
        while let Some(field) = json.next_field()? {
            let repeated = match field.as_ref() {
                "mode" => {
                    let Token::Str(text) = json.expect_token()? else {
                        return Err(json.error("mode is a string"));
                    };
                    let parsed = ShardMode::parse(&text)
                        .ok_or_else(|| json.error("mode is fixed or adaptive"))?;
                    mode.replace(parsed).is_some()
                }
                "declared_shards" => declared.replace(int(&mut json)?).is_some(),
                "next_unit_id" => next.replace(int(&mut json)?).is_some(),
                "units" => units.replace(unit_array(&mut json)?).is_some(),
                "pending_split" => pending.replace(unit_array(&mut json)?).is_some(),
                _ => {
                    json.skip_value()?;
                    false
                }
            };
            if repeated {
                return Err(json.error("a unit layout field is repeated"));
            }
        }
        json.finish()?;
        let (Some(mode), Some(declared_shards), Some(next_unit_id), Some(units)) =
            (mode, declared, next, units)
        else {
            return Err(
                json.error("a unit layout needs mode, declared_shards, next_unit_id and units")
            );
        };
        if declared_shards < 1 || next_unit_id < 1 || units.is_empty() {
            return Err(json.error("a unit layout needs positive counts and a unit"));
        }
        let pending = match pending.as_deref() {
            None | Some([]) => None,
            Some(&[retired, left, right]) => Some([retired, left, right]),
            Some(_) => return Err(json.error("a pending split names three units")),
        };
        Ok(Self {
            mode,
            declared_shards,
            next_unit_id,
            units,
            pending,
        })
    }

    /// Writes the layout as the Java does, into `buf`.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] when `buf` is shorter than the layout:
    /// `actual` is the length it needs.
    pub fn write(&self, buf: &mut [u8]) -> Result<usize, FormatError> {
        let mut out = Out::new(buf);
        out.start_object();
        out.string_field("mode", self.mode.setting_value());
        out.int_field("declared_shards", i64::from(self.declared_shards));
        out.int_field("next_unit_id", i64::from(self.next_unit_id));
        out.start_array_field("units");
        for unit in &self.units {
            write_unit(&mut out, unit);
        }
        out.end_array();
        if let Some(pending) = &self.pending {
            out.start_array_field("pending_split");
            for unit in pending {
                write_unit(&mut out, unit);
            }
            out.end_array();
        }
        out.end_object();
        out.finish(WHAT)
    }
}

fn write_unit(out: &mut Out<'_>, unit: &UnitRecord) {
    // ALLOC: none; 16 hex digits each, on the stack.
    let mut first = [0u8; 16];
    let mut last = [0u8; 16];
    out.start_object();
    out.int_field("id", i64::from(unit.id));
    out.string_field("first", hex(unit.first, &mut first));
    out.string_field("last", hex(unit.last, &mut last));
    out.end_object();
}

/// `Long.toHexString`: lower case, no leading zeros, `0` for zero.
fn hex(n: u64, buf: &mut [u8; 16]) -> &str {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut at = buf.len();
    let mut rest = n;
    loop {
        at = at.saturating_sub(1);
        if let Some(slot) = buf.get_mut(at) {
            // A nibble indexes the sixteen digits.
            *slot = DIGITS.get((rest & 0xf) as usize).copied().unwrap_or(b'0');
        }
        rest >>= 4;
        if rest == 0 {
            break;
        }
    }
    std::str::from_utf8(buf.get(at..).unwrap_or_default()).unwrap_or_default()
}

fn int(json: &mut Reader<'_>) -> Result<i32, FormatError> {
    match json.expect_token()? {
        Token::Int(n) => i32::try_from(n).map_err(|_| json.error("a unit layout count is an int")),
        _ => Err(json.error("a unit layout count is an integer")),
    }
}

fn unit_array(json: &mut Reader<'_>) -> Result<Vec<UnitRecord>, FormatError> {
    if json.expect_token()? != Token::StartArray {
        return Err(json.error("units are an array"));
    }
    // ALLOC: the layout's units, at most MAX_SHARDS, checked per push.
    let mut units = Vec::new();
    loop {
        match json.expect_token()? {
            Token::EndArray => return Ok(units),
            Token::StartObject => {
                if units.len() >= MAX_SHARDS as usize {
                    return Err(FormatError::TooLarge {
                        what: WHAT,
                        limit: u64::from(MAX_SHARDS),
                        actual: u64::from(MAX_SHARDS).saturating_add(1),
                    });
                }
                units.push(unit(json)?);
            }
            _ => return Err(json.error("a unit is an object")),
        }
    }
}

fn unit(json: &mut Reader<'_>) -> Result<UnitRecord, FormatError> {
    let mut id = None;
    let mut first = None;
    let mut last = None;
    while let Some(field) = json.next_field()? {
        let repeated = match field.as_ref() {
            "id" => id.replace(int(json)?).is_some(),
            "first" => first.replace(hash(json)?).is_some(),
            "last" => last.replace(hash(json)?).is_some(),
            _ => {
                json.skip_value()?;
                false
            }
        };
        if repeated {
            return Err(json.error("a unit field is repeated"));
        }
    }
    match (id, first, last) {
        (Some(id), Some(first), Some(last)) if id >= 0 => Ok(UnitRecord { id, first, last }),
        _ => Err(json.error("a unit needs an id that is not negative, first and last")),
    }
}

/// `Long.parseUnsignedLong(text, 16)`: an optional `+`, then hex digits of
/// either case, leading zeros allowed, fitting 64 bits.
fn hash(json: &mut Reader<'_>) -> Result<u64, FormatError> {
    let Token::Str(text) = json.expect_token()? else {
        return Err(json.error("a unit hash is a hex string"));
    };
    u64::from_str_radix(&text, 16).map_err(|_| json.error("a unit hash is hex that fits 64 bits"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_is_javas_to_hex_string() {
        let mut buf = [0u8; 16];
        assert_eq!(hex(0, &mut buf), "0");
        assert_eq!(hex(0xa, &mut buf), "a");
        assert_eq!(hex(0x10, &mut buf), "10");
        assert_eq!(hex(u64::MAX, &mut buf), "ffffffffffffffff");
        assert_eq!(hex(0x0123_4567_89ab_cdef, &mut buf), "123456789abcdef");
    }

    #[test]
    fn modes_parse_as_the_javas() {
        assert_eq!(ShardMode::parse("fixed"), Some(ShardMode::Fixed));
        assert_eq!(ShardMode::parse(" ADAPTIVE\t"), Some(ShardMode::Adaptive));
        assert_eq!(ShardMode::parse("\u{1}fixed\u{20}"), Some(ShardMode::Fixed));
        assert_eq!(ShardMode::parse("\u{a0}fixed"), None);
        assert_eq!(ShardMode::parse("fix"), None);
        assert_eq!(ShardMode::parse(""), None);
    }
}

//! The shell's `StreamInput` primitives over a byte slice: variable-length
//! integers, zigzag longs and strings of UTF-16 characters.

use crate::error::{FormatError, corrupt, len64};

const WHAT: &str = "bucket manifest";

/// The most characters a manifest string may have: names, locations, node
/// ids and uuids are far shorter, and a cap keeps one string referenced by
/// many files from multiplying.
pub(super) const MAX_STRING_CHARS: usize = 1024;

/// A position in a slice; every read checks what is left.
pub(super) struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    pub(super) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    pub(super) fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.at)
    }

    fn truncated(&self) -> FormatError {
        FormatError::Truncated {
            what: WHAT,
            needed: 0,
            have: len64(self.bytes.len()),
        }
    }

    pub(super) fn byte(&mut self) -> Result<u8, FormatError> {
        let b = *self.bytes.get(self.at).ok_or_else(|| self.truncated())?;
        self.at = self.at.saturating_add(1);
        Ok(b)
    }

    /// The next `n` bytes.
    pub(super) fn take(&mut self, n: usize) -> Result<&'a [u8], FormatError> {
        let end = self.at.checked_add(n).ok_or_else(|| self.truncated())?;
        let out = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| self.truncated())?;
        self.at = end;
        Ok(out)
    }

    /// A non-negative `readVLong`: seven bits a byte, at most nine bytes.
    pub(super) fn vlong(&mut self) -> Result<u64, FormatError> {
        let mut value = 0u64;
        for shift in (0..63).step_by(7) {
            let b = self.byte()?;
            value |= u64::from(b & 0x7F) << shift;
            if b & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(corrupt(WHAT, "a variable-length long is past nine bytes"))
    }

    /// A `readVInt` that is a count or an index: non-negative and below 2^31.
    pub(super) fn vint(&mut self) -> Result<usize, FormatError> {
        let value = self.vlong()?;
        if value > u64::from(i32::MAX.unsigned_abs()) {
            return Err(corrupt(WHAT, "a variable-length int is past 31 bits"));
        }
        Ok(usize::try_from(value).unwrap_or(usize::MAX))
    }

    /// A `readZLong`: zigzag over ten bytes.
    pub(super) fn zlong(&mut self) -> Result<i64, FormatError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            value |= u64::from(b & 0x7F) << shift;
            if b & 0x80 == 0 {
                let magnitude = i64::try_from(value >> 1).unwrap_or(i64::MAX);
                return Ok(if value & 1 == 0 {
                    magnitude
                } else {
                    !magnitude
                });
            }
        }
        Err(corrupt(WHAT, "a zigzag long is past ten bytes"))
    }

    /// A `readString`: a character count, then each UTF-16 character as one,
    /// two or three bytes. An unpaired surrogate is refused.
    pub(super) fn string(&mut self) -> Result<String, FormatError> {
        let chars = self.vint()?;
        // Every character takes a byte at least.
        if chars > self.remaining() {
            return Err(self.truncated());
        }
        if chars > MAX_STRING_CHARS {
            return Err(corrupt(WHAT, "a string is past 1024 characters"));
        }
        // ALLOC: one string, at most MAX_STRING_CHARS units.
        let mut units: Vec<u16> = Vec::with_capacity(chars);
        for _ in 0..chars {
            units.push(self.char_unit()?);
        }
        // ALLOC: the decoded string, at most three bytes a unit.
        String::from_utf16(&units).map_err(|_| corrupt(WHAT, "a string has an unpaired surrogate"))
    }

    /// Steps over a string without decoding it, returning where it starts.
    pub(super) fn skip_string(&mut self) -> Result<usize, FormatError> {
        let start = self.at;
        let chars = self.vint()?;
        if chars > self.remaining() {
            return Err(self.truncated());
        }
        if chars > MAX_STRING_CHARS {
            return Err(corrupt(WHAT, "a string is past 1024 characters"));
        }
        for _ in 0..chars {
            let lead = self.byte()?;
            let extra = match lead >> 4 {
                0..=7 => 0,
                12 | 13 => 1,
                14 => 2,
                _ => {
                    return Err(corrupt(
                        WHAT,
                        "a string has a byte that starts no character",
                    ));
                }
            };
            self.take(extra)?;
        }
        Ok(start)
    }

    fn char_unit(&mut self) -> Result<u16, FormatError> {
        let lead = u16::from(self.byte()?);
        match lead >> 4 {
            0..=7 => Ok(lead),
            12 | 13 => {
                let second = u16::from(self.byte()?);
                Ok(((lead & 0x1F) << 6) | (second & 0x3F))
            }
            14 => {
                let second = u16::from(self.byte()?);
                let third = u16::from(self.byte()?);
                Ok(((lead & 0x0F) << 12) | ((second & 0x3F) << 6) | (third & 0x3F))
            }
            _ => Err(corrupt(
                WHAT,
                "a string has a byte that starts no character",
            )),
        }
    }
}

/// Reads the string that starts at `at` in `body`.
pub(super) fn string_at(body: &[u8], at: usize) -> Result<String, FormatError> {
    let mut cursor = Cursor::new(body);
    cursor.take(at)?;
    cursor.string()
}

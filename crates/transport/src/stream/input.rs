//! `StreamInput`'s encodings, read from a slice.

use crate::error::WireError;

/// Reads OpenSearch's stream encodings from a slice, refusing truncated and
/// malformed values. Every length is checked against the bytes left before
/// it sizes anything.
#[derive(Debug, Clone)]
pub struct Input<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Input<'a> {
    /// Reads from the start of `bytes`.
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// Bytes not yet read.
    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.at)
    }

    /// Bytes read so far.
    pub fn position(&self) -> usize {
        self.at
    }

    /// The bytes not yet read.
    pub fn rest(&self) -> &'a [u8] {
        self.bytes.get(self.at..).unwrap_or_default()
    }

    /// Refuses bytes left over, as the Java refuses a message not read to its
    /// end.
    ///
    /// # Errors
    ///
    /// [`WireError::Malformed`] when bytes remain.
    pub fn finish(&self, what: &'static str) -> Result<(), WireError> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(WireError::malformed(what, "bytes remain after the message"))
        }
    }

    /// `readBytes`: `len` raw bytes.
    ///
    /// # Errors
    ///
    /// [`WireError::Truncated`] when fewer remain.
    pub fn take(&mut self, len: usize, what: &'static str) -> Result<&'a [u8], WireError> {
        let end = self
            .at
            .checked_add(len)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(WireError::Truncated { what })?;
        let out = self
            .bytes
            .get(self.at..end)
            .ok_or(WireError::Truncated { what })?;
        self.at = end;
        Ok(out)
    }

    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], WireError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N, what)?);
        Ok(out)
    }

    /// `readByte`.
    ///
    /// # Errors
    ///
    /// [`WireError::Truncated`] at the end.
    pub fn byte(&mut self, what: &'static str) -> Result<u8, WireError> {
        let [b] = self.array::<1>(what)?;
        Ok(b)
    }

    /// `readBoolean`: 0 or 1, anything else refused.
    ///
    /// # Errors
    ///
    /// [`WireError::Truncated`] at the end; [`WireError::Malformed`] for
    /// another byte.
    pub fn bool(&mut self, what: &'static str) -> Result<bool, WireError> {
        match self.byte(what)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(WireError::malformed(what, "a boolean is 0 or 1")),
        }
    }

    /// `readInt`: four bytes, big-endian.
    ///
    /// # Errors
    ///
    /// [`WireError::Truncated`] when fewer remain.
    pub fn int(&mut self, what: &'static str) -> Result<i32, WireError> {
        Ok(i32::from_be_bytes(self.array(what)?))
    }

    /// `readLong`: eight bytes, big-endian.
    ///
    /// # Errors
    ///
    /// [`WireError::Truncated`] when fewer remain.
    pub fn long(&mut self, what: &'static str) -> Result<i64, WireError> {
        Ok(i64::from_be_bytes(self.array(what)?))
    }

    /// `readVInt`: at most five bytes, the fifth without a continuation bit.
    ///
    /// # Errors
    ///
    /// [`WireError::Truncated`] at the end; [`WireError::Malformed`] past
    /// five bytes.
    pub fn vint(&mut self, what: &'static str) -> Result<i32, WireError> {
        let mut value = 0u32;
        for shift in [0u32, 7, 14, 21, 28] {
            let b = self.byte(what)?;
            if shift == 28 && b & 0x80 != 0 {
                return Err(WireError::malformed(what, "a vint is at most five bytes"));
            }
            value |= u32::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                break;
            }
        }
        Ok(i32::from_ne_bytes(value.to_ne_bytes()))
    }

    /// `readVLong`: seven bits a byte, at most nine; a tenth byte, 0 or 1,
    /// supplies bit 63 (written only by `writeVLongNoCheck`).
    ///
    /// # Errors
    ///
    /// [`WireError::Truncated`] at the end; [`WireError::Malformed`] for a
    /// tenth byte above 1.
    pub fn vlong(&mut self, what: &'static str) -> Result<i64, WireError> {
        let mut value = 0u64;
        for shift in [0u32, 7, 14, 21, 28, 35, 42, 49, 56] {
            let b = self.byte(what)?;
            value |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(i64::from_ne_bytes(value.to_ne_bytes()));
            }
        }
        match self.byte(what)? {
            0 => {}
            1 => value |= 1 << 63,
            _ => return Err(WireError::malformed(what, "a vlong's tenth byte is 0 or 1")),
        }
        Ok(i64::from_ne_bytes(value.to_ne_bytes()))
    }

    /// `readZLong`: seven bits a byte, at most ten bytes, then zig-zag.
    ///
    /// # Errors
    ///
    /// [`WireError::Truncated`] at the end; [`WireError::Malformed`] past
    /// ten bytes.
    pub fn zlong(&mut self, what: &'static str) -> Result<i64, WireError> {
        let mut value = 0u64;
        let mut shift = 0u32;
        loop {
            let b = self.byte(what)?;
            if b & 0x80 == 0 {
                value |= u64::from(b).checked_shl(shift).unwrap_or(0);
                break;
            }
            value |= u64::from(b & 0x7f) << shift;
            shift = shift.saturating_add(7);
            if shift > 63 {
                return Err(WireError::malformed(what, "a zlong is at most ten bytes"));
            }
        }
        // Zig-zag: the low bit is the sign, all ones when set.
        let sign = 0u64.wrapping_sub(value & 1);
        Ok(i64::from_ne_bytes(((value >> 1) ^ sign).to_ne_bytes()))
    }

    /// A count or length (`readArraySize`): a vint that is not negative and
    /// not more than the bytes left, each element taking at least `min_each`
    /// bytes.
    ///
    /// # Errors
    ///
    /// [`WireError::Malformed`] for a negative count; [`WireError::Truncated`]
    /// for one the bytes left cannot hold.
    pub fn count(&mut self, min_each: usize, what: &'static str) -> Result<usize, WireError> {
        let count = usize::try_from(self.vint(what)?)
            .map_err(|_| WireError::malformed(what, "a count is not negative"))?;
        if count.saturating_mul(min_each) > self.remaining() {
            return Err(WireError::Truncated { what });
        }
        Ok(count)
    }

    /// `readString`: a count of UTF-16 chars, then each in one to three
    /// bytes. A lone surrogate, which a Rust string cannot hold, is refused.
    ///
    /// # Errors
    ///
    /// [`WireError::Truncated`] when the bytes end first;
    /// [`WireError::Malformed`] for a byte no char starts with, or a lone
    /// surrogate.
    pub fn string(&mut self, what: &'static str) -> Result<String, WireError> {
        let chars = self.count(1, what)?;
        // ALLOC: at most one UTF-16 unit per remaining input byte (checked by `count`).
        let mut units = Vec::with_capacity(chars);
        for _ in 0..chars {
            let c = u16::from(self.byte(what)?);
            let unit = match c >> 4 {
                0..=7 => c,
                12 | 13 => ((c & 0x1f) << 6) | self.continuation(what)?,
                14 => {
                    let high = self.continuation(what)?;
                    ((c & 0x0f) << 12) | (high << 6) | self.continuation(what)?
                }
                _ => return Err(WireError::malformed(what, "a byte no char starts with")),
            };
            units.push(unit);
        }
        String::from_utf16(&units).map_err(|_| WireError::malformed(what, "a lone surrogate"))
    }

    fn continuation(&mut self, what: &'static str) -> Result<u16, WireError> {
        // The Java masks the low six bits without checking the top two.
        Ok(u16::from(self.byte(what)? & 0x3f))
    }

    /// `readOptionalString`: a boolean, then the string when it is true.
    ///
    /// # Errors
    ///
    /// As [`Self::bool`] and [`Self::string`].
    pub fn optional_string(&mut self, what: &'static str) -> Result<Option<String>, WireError> {
        if self.bool(what)? {
            self.string(what).map(Some)
        } else {
            Ok(None)
        }
    }

    /// `readStringArray`: a count, then each string.
    ///
    /// # Errors
    ///
    /// As [`Self::count`] and [`Self::string`].
    pub fn string_array(&mut self, what: &'static str) -> Result<Vec<String>, WireError> {
        let count = self.count(1, what)?;
        // ALLOC: at most one string per remaining input byte (checked by `count`).
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(self.string(what)?);
        }
        Ok(out)
    }

    /// `readMap(readString, readString)`: a count, then each key and value,
    /// in wire order.
    ///
    /// # Errors
    ///
    /// As [`Self::count`] and [`Self::string`].
    pub fn string_map(&mut self, what: &'static str) -> Result<Vec<(String, String)>, WireError> {
        let count = self.count(2, what)?;
        // ALLOC: at most one entry per two remaining input bytes (checked by `count`).
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let key = self.string(what)?;
            out.push((key, self.string(what)?));
        }
        Ok(out)
    }

    /// `readBytesReference`: a length, then that many bytes, borrowed.
    ///
    /// # Errors
    ///
    /// As [`Self::count`].
    pub fn bytes_reference(&mut self, what: &'static str) -> Result<&'a [u8], WireError> {
        let len = self.count(1, what)?;
        self.take(len, what)
    }
}

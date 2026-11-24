//! `StreamOutput`'s encodings, written into a caller's buffer up to a limit.

use crate::error::WireError;

/// Writes OpenSearch's stream encodings into a buffer, refusing to grow it
/// past `limit` bytes in all.
#[derive(Debug)]
pub struct Output<'a> {
    buf: &'a mut Vec<u8>,
    limit: usize,
}

impl<'a> Output<'a> {
    /// Writes after what `buf` already holds, up to `limit` bytes in all.
    pub fn new(buf: &'a mut Vec<u8>, limit: usize) -> Self {
        Self { buf, limit }
    }

    /// Bytes in the buffer, including any held before this writer.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// The bytes written so far, including any held before this writer.
    pub fn as_slice(&self) -> &[u8] {
        self.buf
    }

    /// `write(byte[])`: raw bytes, no length.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the limit; nothing is written then.
    pub fn bytes(&mut self, bytes: &[u8]) -> Result<(), WireError> {
        let wanted = self.buf.len().saturating_add(bytes.len());
        if wanted > self.limit {
            return Err(WireError::too_large(
                "a transport message",
                self.limit,
                wanted,
            ));
        }
        // ALLOC: grows the caller's buffer, never past `limit`.
        self.buf.extend_from_slice(bytes);
        Ok(())
    }

    /// `writeByte`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the limit.
    pub fn byte(&mut self, value: u8) -> Result<(), WireError> {
        self.bytes(&[value])
    }

    /// `writeBoolean`: 1 or 0.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the limit.
    pub fn bool(&mut self, value: bool) -> Result<(), WireError> {
        self.byte(u8::from(value))
    }

    /// `writeInt`: four bytes, big-endian.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the limit.
    pub fn int(&mut self, value: i32) -> Result<(), WireError> {
        self.bytes(&value.to_be_bytes())
    }

    /// `writeLong`: eight bytes, big-endian.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the limit.
    pub fn long(&mut self, value: i64) -> Result<(), WireError> {
        self.bytes(&value.to_be_bytes())
    }

    /// `writeVInt`: seven bits a byte, low first; a negative value takes
    /// five bytes, as the Java writes it.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the limit.
    pub fn vint(&mut self, value: i32) -> Result<(), WireError> {
        self.varint(u64::from(u32::from_ne_bytes(value.to_ne_bytes())))
    }

    /// `writeVLong`: a value that is not negative, seven bits a byte.
    ///
    /// # Errors
    ///
    /// [`WireError::Malformed`] for a negative value, which the Java refuses
    /// to write; [`WireError::TooLarge`] past the limit.
    pub fn vlong(&mut self, value: i64) -> Result<(), WireError> {
        let value = u64::try_from(value)
            .map_err(|_| WireError::malformed("a vlong", "negative values are written as zlong"))?;
        self.varint(value)
    }

    /// `writeZLong`: zig-zag, then seven bits a byte.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the limit.
    pub fn zlong(&mut self, value: i64) -> Result<(), WireError> {
        let zigzag = (value << 1) ^ (value >> 63);
        self.varint(u64::from_ne_bytes(zigzag.to_ne_bytes()))
    }

    fn varint(&mut self, mut value: u64) -> Result<(), WireError> {
        let mut out = [0u8; 10];
        let mut used = 0usize;
        for slot in &mut out {
            used = used.saturating_add(1);
            let low = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                *slot = low;
                break;
            }
            *slot = low | 0x80;
        }
        self.bytes(out.get(..used).unwrap_or_default())
    }

    /// `writeString`: the count of UTF-16 chars as a vint, then each char in
    /// one byte below `0x80`, two below `0x800`, three otherwise (a
    /// surrogate pair as two three-byte chars).
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the limit, or for a string of more chars
    /// than a Java string holds.
    pub fn string(&mut self, value: &str) -> Result<(), WireError> {
        if value.is_ascii() {
            // One byte a char, the same bytes the loop below writes.
            let count = i32::try_from(value.len())
                .map_err(|_| WireError::too_large("a string", i32::MAX as usize, value.len()))?;
            self.vint(count)?;
            return self.bytes(value.as_bytes());
        }
        let chars = value.encode_utf16().count();
        let count = i32::try_from(chars)
            .map_err(|_| WireError::too_large("a string", i32::MAX as usize, chars))?;
        self.vint(count)?;
        for unit in value.encode_utf16() {
            let c = u32::from(unit);
            if c <= 0x7f {
                self.byte(c as u8)?;
            } else if c > 0x7ff {
                self.bytes(&[
                    0xe0 | ((c >> 12) & 0x0f) as u8,
                    0x80 | ((c >> 6) & 0x3f) as u8,
                    0x80 | (c & 0x3f) as u8,
                ])?;
            } else {
                self.bytes(&[0xc0 | ((c >> 6) & 0x1f) as u8, 0x80 | (c & 0x3f) as u8])?;
            }
        }
        Ok(())
    }

    /// `writeOptionalString`: a boolean, then the string when present.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the limit.
    pub fn optional_string(&mut self, value: Option<&str>) -> Result<(), WireError> {
        match value {
            None => self.bool(false),
            Some(text) => {
                self.bool(true)?;
                self.string(text)
            }
        }
    }

    /// `writeStringArray` (and `writeStringCollection`): a count, then each.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the limit.
    pub fn string_array(&mut self, values: &[&str]) -> Result<(), WireError> {
        self.count(values.len())?;
        for value in values {
            self.string(value)?;
        }
        Ok(())
    }

    /// `writeMap(map, writeString, writeString)`: a count, then each key and
    /// value, in the order given (the Java writes a map's iteration order).
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the limit.
    pub fn string_map<K: AsRef<str>, V: AsRef<str>>(
        &mut self,
        entries: &[(K, V)],
    ) -> Result<(), WireError> {
        self.count(entries.len())?;
        for (key, value) in entries {
            self.string(key.as_ref())?;
            self.string(value.as_ref())?;
        }
        Ok(())
    }

    /// `writeBytesReference`: a vint length, then the bytes.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the limit.
    pub fn bytes_reference(&mut self, bytes: &[u8]) -> Result<(), WireError> {
        self.count(bytes.len())?;
        self.bytes(bytes)
    }

    /// A count or length as a vint.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the limit or past `i32::MAX`.
    pub fn count(&mut self, count: usize) -> Result<(), WireError> {
        let value = i32::try_from(count)
            .map_err(|_| WireError::too_large("a count", i32::MAX as usize, count))?;
        self.vint(value)
    }
}
